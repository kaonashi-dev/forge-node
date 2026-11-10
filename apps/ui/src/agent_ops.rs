//! Session-to-session gestures that need a daemon request: continuation
//! handoff, spawning a child, and sending a context envelope. Every capture
//! and launch runs on the client worker; these screens only keep form state.

use domain::{
    AgentProfileId, AgentProviderId, ChildWorkspacePolicy, SessionId, SessionKind, SessionRole,
    WorkspaceId,
};
use rune::{El, TextEdit, col, el, row, text};
use rune_kit::{
    Button, ButtonVariant, Checkbox, ControlSize, Dialog, Kit, SegmentedControl, TextArea,
    TextInput,
};

use crate::app::AppState;
use crate::handoff::{self, Source};

/// The daemon clamps both again; these are the capture the dialog asks for.
pub const TRANSCRIPT_MAX_LINES: u32 = 4_096;
pub const TRANSCRIPT_MAX_BYTES: u32 = 96_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub key: String,
    pub label: String,
    pub provider: AgentProviderId,
    pub profile: Option<AgentProfileId>,
}

/// What the launch pickers offer, in the original client's order: each provider and the
/// profiles under it. Only rows that can start, and only prompt-taking rows
/// when the launch has to carry a first prompt.
#[must_use]
pub fn choices(store: &client::Store, prompt_only: bool) -> Vec<Choice> {
    let visible =
        |key: &str| store.app_state_value(&crate::launch::visibility_key(key)) != Some("false");
    let mut out = Vec::new();
    for info in &store.providers {
        let provider = &info.descriptor.id;
        let installed = matches!(
            info.detection.status,
            domain::DetectionStatus::Installed { .. }
        );
        let takes_prompt = info.descriptor.capabilities.supports_initial_prompt;
        if installed && visible(provider.as_str()) && (!prompt_only || takes_prompt) {
            out.push(Choice {
                key: format!("provider:{provider}"),
                label: info.descriptor.display_name.clone(),
                provider: provider.clone(),
                profile: None,
            });
        }
        for profile in store
            .agent_profiles
            .iter()
            .filter(|profile| profile.provider_id == *provider)
        {
            if !visible(&format!("profile:{}", profile.id)) {
                continue;
            }
            let launchable = installed || profile.executable.is_some();
            if launchable && (!prompt_only || takes_prompt) {
                out.push(Choice {
                    key: format!("profile:{}", profile.id),
                    label: profile.name.clone(),
                    provider: provider.clone(),
                    profile: Some(profile.id),
                });
            }
        }
    }
    out
}

/// The source's own account first, then its bare provider, then whatever can
/// start at all.
#[must_use]
pub fn initial_choice<'a>(
    rows: &'a [Choice],
    provider: Option<&AgentProviderId>,
    profile: Option<AgentProfileId>,
) -> Option<&'a Choice> {
    if let Some(profile) = profile
        && let Some(row) = rows.iter().find(|row| {
            row.profile == Some(profile)
                && provider.is_none_or(|provider| &row.provider == provider)
        })
    {
        return Some(row);
    }
    if let Some(provider) = provider
        && let Some(row) = rows
            .iter()
            .find(|row| &row.provider == provider && row.profile.is_none())
    {
        return Some(row);
    }
    rows.first()
}

#[must_use]
pub fn selected<'a>(rows: &'a [Choice], key: Option<&str>) -> Option<&'a Choice> {
    key.and_then(|key| rows.iter().find(|row| row.key == key))
        .or_else(|| rows.first())
}

pub const ROLES: [(SessionRole, &str); 6] = [
    (SessionRole::Generic, "Generic"),
    (SessionRole::Planner, "Planner"),
    (SessionRole::Researcher, "Researcher"),
    (SessionRole::Executor, "Executor"),
    (SessionRole::Reviewer, "Reviewer"),
    (SessionRole::Tester, "Tester"),
];

/// the original client's `SendContextDialog` offers the four roles a context hand-off needs.
pub const CONTEXT_ROLES: [(SessionRole, &str); 4] = [
    (SessionRole::Generic, "Generic"),
    (SessionRole::Planner, "Planner"),
    (SessionRole::Executor, "Executor"),
    (SessionRole::Reviewer, "Reviewer"),
];

pub enum Capture {
    Loading,
    Ready {
        text: String,
        lines: u32,
        truncated: bool,
    },
    Failed(String),
}

pub struct Handoff {
    pub session: SessionId,
    pub workspace: WorkspaceId,
    pub title: String,
    pub source_label: Option<String>,
    pub working_directory: String,
    pub branch: Option<String>,
    pub capture: Capture,
    pub prompt: TextEdit,
    pub focus: TextEdit,
    pub prompt_edited: bool,
    pub target: Option<String>,
    pub error: Option<String>,
    pub starting: bool,
}

impl Handoff {
    fn source(&self) -> Source<'_> {
        Source {
            agent: self.source_label.as_deref(),
            title: Some(self.title.as_str()),
            working_directory: &self.working_directory,
            branch: self.branch.as_deref(),
        }
    }

    fn rebuild_prompt(&mut self) {
        let Capture::Ready {
            text, truncated, ..
        } = &self.capture
        else {
            return;
        };
        if let Some(prompt) = handoff::continuation_prompt(
            &self.source(),
            text,
            *truncated,
            Some(self.focus.text.as_str()),
        ) {
            self.prompt = TextEdit::new(prompt);
            self.prompt_edited = false;
        }
    }
}

pub struct SpawnChild {
    pub parent: SessionId,
    pub title: String,
    pub target: Option<String>,
    pub role: usize,
    pub workspace: usize,
    pub prompt: TextEdit,
    pub error: Option<String>,
    pub starting: bool,
}

pub struct SendContext {
    pub source: SessionId,
    pub title: String,
    pub spawn: bool,
    pub target_session: Option<SessionId>,
    pub target: Option<String>,
    pub role: usize,
    pub summary: TextEdit,
    pub instructions: TextEdit,
    pub include_transcript: bool,
    pub error: Option<String>,
    pub starting: bool,
}

#[derive(Default)]
pub struct AgentOps {
    pub handoff: Option<Handoff>,
    pub spawn: Option<SpawnChild>,
    pub send_context: Option<SendContext>,
}

pub fn open_handoff(s: &mut AppState, session_id: SessionId) {
    let Some(store) = &s.store else {
        return;
    };
    let Some(session) = store
        .sessions
        .iter()
        .find(|session| session.id == session_id)
    else {
        return;
    };
    let Some(workspace) = store
        .workspaces
        .iter()
        .find(|workspace| workspace.id == session.workspace_id)
    else {
        return;
    };
    let title = s.rows.iter().find(|row| row.id == session_id).map_or_else(
        || crate::session_label::session_title(session),
        |row| row.title.clone(),
    );
    let source_label = session.agent_provider_id.as_ref().map(|provider| {
        store
            .providers
            .iter()
            .find(|info| info.descriptor.id == *provider)
            .map_or_else(
                || provider.to_string(),
                |info| info.descriptor.display_name.clone(),
            )
    });
    let rows = choices(store, true);
    let target = initial_choice(
        &rows,
        session.agent_provider_id.as_ref(),
        session.agent_profile_id,
    )
    .map(|row| row.key.clone());
    let workspace_id = workspace.id;
    let working_directory = workspace.path.display().to_string();
    let branch = workspace.branch.clone();
    s.agent_ops.handoff = Some(Handoff {
        session: session_id,
        workspace: workspace_id,
        title,
        source_label,
        working_directory,
        branch,
        capture: Capture::Loading,
        prompt: TextEdit::new(""),
        focus: TextEdit::new(""),
        prompt_edited: false,
        target,
        error: None,
        starting: false,
    });
    match &s.connection {
        Some(connection) => {
            if let Err(error) = connection.session_transcript(
                s.generation,
                session_id,
                Some(TRANSCRIPT_MAX_LINES),
                Some(TRANSCRIPT_MAX_BYTES),
            ) && let Some(handoff) = s.agent_ops.handoff.as_mut()
            {
                handoff.capture = Capture::Failed(error);
            }
        }
        None => {
            if let Some(handoff) = s.agent_ops.handoff.as_mut() {
                handoff.capture = Capture::Failed("Not connected to the daemon.".into());
            }
        }
    }
}

pub fn open_spawn_child(s: &mut AppState, session_id: SessionId) {
    let Some(store) = &s.store else {
        return;
    };
    let Some(session) = store
        .sessions
        .iter()
        .find(|session| session.id == session_id)
    else {
        return;
    };
    let title = s.rows.iter().find(|row| row.id == session_id).map_or_else(
        || crate::session_label::session_title(session),
        |row| row.title.clone(),
    );
    let rows = choices(store, false);
    s.agent_ops.spawn = Some(SpawnChild {
        parent: session_id,
        title,
        target: rows.first().map(|row| row.key.clone()),
        role: 0,
        workspace: 0,
        prompt: TextEdit::new(""),
        error: None,
        starting: false,
    });
}

pub fn open_send_context(s: &mut AppState, session_id: SessionId) {
    let Some(store) = &s.store else {
        return;
    };
    let Some(session) = store
        .sessions
        .iter()
        .find(|session| session.id == session_id)
    else {
        return;
    };
    let title = s.rows.iter().find(|row| row.id == session_id).map_or_else(
        || crate::session_label::session_title(session),
        |row| row.title.clone(),
    );
    let target_session = store
        .sessions
        .iter()
        .find(|candidate| candidate.id != session_id)
        .map(|candidate| candidate.id);
    let rows = choices(store, true);
    s.agent_ops.send_context = Some(SendContext {
        source: session_id,
        title,
        spawn: false,
        target_session,
        target: rows.first().map(|row| row.key.clone()),
        role: 0,
        summary: TextEdit::new(""),
        instructions: TextEdit::new(""),
        include_transcript: true,
        error: None,
        starting: false,
    });
}

pub fn apply_transcript(
    s: &mut AppState,
    session_id: SessionId,
    result: Result<domain::SessionTranscript, String>,
) -> bool {
    let Some(handoff) = s.agent_ops.handoff.as_mut() else {
        return false;
    };
    if handoff.session != session_id {
        return false;
    }
    match result {
        Ok(transcript) => {
            handoff.capture = Capture::Ready {
                text: transcript.text,
                lines: transcript.lines,
                truncated: transcript.truncated,
            };
            handoff.rebuild_prompt();
        }
        Err(error) => handoff.capture = Capture::Failed(error),
    }
    true
}

pub fn start_handoff(s: &mut AppState) {
    let Some(store) = &s.store else {
        return;
    };
    let rows = choices(store, true);
    let Some(handoff) = s.agent_ops.handoff.as_mut() else {
        return;
    };
    let Some(choice) = selected(&rows, handoff.target.as_deref()) else {
        handoff.error = Some("No installed agent can take a prompt at launch.".into());
        return;
    };
    if !matches!(handoff.capture, Capture::Ready { .. }) {
        handoff.error = Some("This session's transcript has not been read yet.".into());
        return;
    }
    let prompt = handoff.prompt.text.trim().to_string();
    if prompt.is_empty() {
        handoff.error = Some("Write the prompt the new session should start with.".into());
        return;
    }
    let workspace = handoff.workspace;
    let parent = handoff.session;
    let provider = choice.provider.clone();
    let profile = choice.profile;
    let result = match &s.connection {
        Some(connection) => connection.continue_agent(workspace, provider, profile, parent, prompt),
        None => Err("The client worker is still starting.".to_string()),
    };
    match result {
        Ok(()) => {
            handoff.starting = true;
            handoff.error = None;
        }
        Err(error) => handoff.error = Some(error),
    }
}

pub fn start_spawn_child(s: &mut AppState) {
    let Some(store) = &s.store else {
        return;
    };
    let rows = choices(store, false);
    let Some(spawn) = s.agent_ops.spawn.as_mut() else {
        return;
    };
    let Some(choice) = selected(&rows, spawn.target.as_deref()) else {
        spawn.error = Some("No installed agent is available to launch.".into());
        return;
    };
    let role = ROLES
        .get(spawn.role)
        .map_or(SessionRole::Generic, |(role, _)| role.clone());
    let policy = if spawn.workspace == 1 {
        ChildWorkspacePolicy::NewManagedWorktree {
            branch_hint: None,
            base: None,
        }
    } else {
        ChildWorkspacePolicy::SameWorkspace
    };
    let prompt = spawn.prompt.text.trim();
    let prompt = (!prompt.is_empty()).then(|| prompt.to_string());
    let parent = spawn.parent;
    let provider = choice.provider.clone();
    let profile = choice.profile;
    let result = match &s.connection {
        Some(connection) => {
            connection.create_child(parent, provider, profile, role, policy, prompt)
        }
        None => Err("The client worker is still starting.".to_string()),
    };
    match result {
        Ok(()) => {
            spawn.starting = true;
            spawn.error = None;
        }
        Err(error) => spawn.error = Some(error),
    }
}

pub fn start_send_context(s: &mut AppState) {
    let Some(store) = &s.store else {
        return;
    };
    let rows = choices(store, true);
    let Some(send) = s.agent_ops.send_context.as_mut() else {
        return;
    };
    let summary = send.summary.text.trim();
    let instructions = send.instructions.text.trim();
    if summary.is_empty() && instructions.is_empty() && !send.include_transcript {
        send.error =
            Some("Say something to send: a summary, instructions, or the transcript.".into());
        return;
    }
    let summary = (!summary.is_empty()).then(|| summary.to_string());
    let instructions = (!instructions.is_empty()).then(|| instructions.to_string());
    let source = send.source;
    let include = send.include_transcript;
    let result = if send.spawn {
        let Some(choice) = selected(&rows, send.target.as_deref()) else {
            send.error = Some("No installed agent can take a launch prompt.".into());
            return;
        };
        let role = CONTEXT_ROLES
            .get(send.role)
            .map_or(SessionRole::Generic, |(role, _)| role.clone());
        let spawn = client::SendContextSpawn {
            kind: SessionKind::Agent,
            provider_id: Some(choice.provider.clone()),
            profile_id: choice.profile,
            role,
            workspace_policy: ChildWorkspacePolicy::SameWorkspace,
        };
        match &s.connection {
            Some(connection) => connection.send_context(
                source,
                None,
                Some(spawn),
                summary,
                instructions,
                include,
                Some(TRANSCRIPT_MAX_BYTES),
            ),
            None => Err("The client worker is still starting.".to_string()),
        }
    } else {
        let Some(target) = send.target_session else {
            send.error = Some("Pick the session this context is for.".into());
            return;
        };
        match &s.connection {
            Some(connection) => connection.send_context(
                source,
                Some(target),
                None,
                summary,
                instructions,
                include,
                Some(TRANSCRIPT_MAX_BYTES),
            ),
            None => Err("The client worker is still starting.".to_string()),
        }
    };
    match result {
        Ok(()) => {
            send.starting = true;
            send.error = None;
        }
        Err(error) => send.error = Some(error),
    }
}

pub fn command_result(s: &mut AppState, label: &str, ok: bool, message: &str) {
    match label {
        "CreateAgentSession" => {
            if let Some(handoff) = s.agent_ops.handoff.as_mut()
                && handoff.starting
            {
                if ok {
                    s.agent_ops.handoff = None;
                } else {
                    handoff.starting = false;
                    handoff.error = Some(message.to_string());
                }
            }
        }
        "CreateChildSession" => {
            if let Some(spawn) = s.agent_ops.spawn.as_mut()
                && spawn.starting
            {
                if ok {
                    s.agent_ops.spawn = None;
                } else {
                    spawn.starting = false;
                    spawn.error = Some(message.to_string());
                }
            }
        }
        "SendContext" => {
            if let Some(send) = s.agent_ops.send_context.as_mut()
                && send.starting
            {
                if ok {
                    s.agent_ops.send_context = None;
                } else {
                    send.starting = false;
                    send.error = Some(message.to_string());
                }
            }
        }
        _ => {}
    }
}

#[derive(Clone, Copy)]
enum Picker {
    Handoff,
    Spawn,
    SendContext,
}

fn choose_agent(s: &mut AppState, picker: Picker, key: String) {
    match picker {
        Picker::Handoff => {
            if let Some(handoff) = s.agent_ops.handoff.as_mut() {
                handoff.target = Some(key);
            }
        }
        Picker::Spawn => {
            if let Some(spawn) = s.agent_ops.spawn.as_mut() {
                spawn.target = Some(key);
            }
        }
        Picker::SendContext => {
            if let Some(send) = s.agent_ops.send_context.as_mut() {
                send.target = Some(key);
            }
        }
    }
}

fn agent_picker(
    k: &Kit,
    rows: &[Choice],
    selected: Option<&str>,
    prefix: &str,
    picker: Picker,
) -> El {
    let mut list = col().key(format!("{prefix}-agents")).w_full().gap(2.0);
    if rows.is_empty() {
        return list.child(
            text("No installed agent is available to launch.")
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint),
        );
    }
    for (index, row) in rows.iter().enumerate() {
        let current = Some(row.key.as_str()) == selected || selected.is_none() && index == 0;
        let key = row.key.clone();
        list = list.child(
            Button::new(row.label.clone())
                .variant(if current {
                    ButtonVariant::Outline
                } else {
                    ButtonVariant::Ghost
                })
                .control_size(ControlSize::Small)
                .on_press(move |s: &mut AppState| choose_agent(s, picker, key.clone()))
                .build(k)
                .key(format!("{prefix}-agent-{index}")),
        );
    }
    list
}

fn footer(
    k: &Kit,
    cancel_key: &'static str,
    cancel: fn(&mut AppState),
    start_label: &'static str,
    start_key: &'static str,
    start: fn(&mut AppState),
    disabled: bool,
) -> El {
    row()
        .pt(12.0)
        .gap(8.0)
        .child(el().grow(1.0))
        .child(
            Button::new("Cancel")
                .variant(ButtonVariant::Ghost)
                .on_press(cancel)
                .build(k)
                .key(cancel_key),
        )
        .child(
            Button::new(start_label)
                .variant(ButtonVariant::Contrast)
                .disabled(disabled)
                .on_press(start)
                .build(k)
                .key(start_key),
        )
}

fn capture_line(handoff: &Handoff, k: &Kit) -> Option<El> {
    match &handoff.capture {
        Capture::Loading => Some(
            text("Reading this session's terminal…")
                .key("handoff-capture")
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint),
        ),
        Capture::Failed(error) => Some(
            text(format!("Could not read the transcript: {error}"))
                .key("handoff-capture-error")
                .role("alert")
                .font(k.ty.text_xsmall)
                .color(k.c.danger.fg),
        ),
        Capture::Ready {
            lines, truncated, ..
        } => Some(
            text(if *truncated {
                format!(
                    "Captured {lines} lines; older output was dropped to stay inside the budget, and the prompt says so."
                )
            } else {
                format!("Captured {lines} lines.")
            })
            .key("handoff-capture")
            .font(k.ty.text_xsmall)
            .color(k.c.text_faint),
        ),
    }
}

fn handoff_dialog(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    let Some(handoff) = &s.agent_ops.handoff else {
        return;
    };
    let rows = s
        .store
        .as_ref()
        .map_or_else(Vec::new, |store| choices(store, true));
    let provider = selected(&rows, handoff.target.as_deref());
    let mut body = col()
        .w_full()
        .gap(10.0)
        .child(
            text(
                "A fresh agent starts from this session's stopping point, in the same checkout. \
                 The prompt below is what it receives.",
            )
            .font(k.ty.small)
            .color(k.c.text_muted)
            .wrap(),
        )
        .child(
            row()
                .key("handoff-source")
                .gap(6.0)
                .items_center()
                .child(
                    text(handoff.title.clone())
                        .font(k.ty.small_medium)
                        .color(k.c.text_base),
                )
                .child(
                    text(handoff.working_directory.clone())
                        .font(k.ty.text_xsmall)
                        .color(k.c.text_faint)
                        .truncate(),
                ),
        )
        .children(capture_line(handoff, k))
        .child(agent_picker(
            k,
            &rows,
            handoff.target.as_deref(),
            "handoff",
            Picker::Handoff,
        ));
    body = body.child(
        TextInput::new(&handoff.focus)
            .label("Focus (optional)")
            .placeholder("Continue with the implementation focusing on…")
            .on_edit(|s: &mut AppState| {
                &mut s
                    .agent_ops
                    .handoff
                    .as_mut()
                    .expect("the handoff dialog is open")
                    .focus
            })
            .on_change(|s: &mut AppState, _| {
                if let Some(handoff) = s.agent_ops.handoff.as_mut()
                    && !handoff.prompt_edited
                {
                    handoff.rebuild_prompt();
                }
            })
            .build(k)
            .key("handoff-focus"),
    );
    body = body.child(
        TextArea::new(&handoff.prompt)
            .label("Continuation prompt")
            .on_edit(|s: &mut AppState| {
                &mut s
                    .agent_ops
                    .handoff
                    .as_mut()
                    .expect("the handoff dialog is open")
                    .prompt
            })
            .on_change(|s: &mut AppState, _| {
                if let Some(handoff) = s.agent_ops.handoff.as_mut() {
                    handoff.prompt_edited = true;
                }
            })
            .build(k)
            .key("handoff-prompt")
            .h(220.0),
    );
    if let Some(error) = &handoff.error {
        body = body.child(
            text(error.clone())
                .key("handoff-error")
                .role("alert")
                .font(k.ty.text_xsmall)
                .color(k.c.danger.fg),
        );
    }
    let start_label = if handoff.starting {
        "Starting…"
    } else {
        "Start new session"
    };
    let ready = provider.is_some()
        && !handoff.starting
        && matches!(handoff.capture, Capture::Ready { .. })
        && !handoff.prompt.text.trim().is_empty();
    Dialog::new("handoff", "Continue in a new session")
        .open(true)
        .fit()
        .width(680.0)
        .on_close(|s: &mut AppState| s.agent_ops.handoff = None)
        .child(body)
        .child(footer(
            k,
            "handoff-close",
            |s| s.agent_ops.handoff = None,
            start_label,
            "handoff-start",
            start_handoff,
            !ready,
        ))
        .build(k, layer);
}

fn role_picker(
    k: &Kit,
    roles: &[(SessionRole, &str)],
    selected: usize,
    key: &'static str,
    pick: fn(&mut AppState, usize),
) -> El {
    let labels: Vec<&str> = roles.iter().map(|(_, label)| *label).collect();
    SegmentedControl::new(&labels, Some(selected))
        .label("Role")
        .fit()
        .on_pick(move |s: &mut AppState, index: usize, _| pick(s, index))
        .build(k)
        .key(key)
}

fn spawn_dialog(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    let Some(spawn) = &s.agent_ops.spawn else {
        return;
    };
    let rows = s
        .store
        .as_ref()
        .map_or_else(Vec::new, |store| choices(store, false));
    let provider = selected(&rows, spawn.target.as_deref());
    let mut body = col()
        .w_full()
        .gap(10.0)
        .child(
            text(format!(
                "Starts a nested agent under {}. Providers talk only through Forge, so this is how one session asks another for help.",
                spawn.title
            ))
            .font(k.ty.small)
            .color(k.c.text_muted)
            .wrap(),
        )
        .child(agent_picker(
            k,
            &rows,
            spawn.target.as_deref(),
            "spawn-child",
            Picker::Spawn,
        ))
        .child(role_picker(
            k,
            &ROLES,
            spawn.role,
            "spawn-child-role",
            |s, index| {
                if let Some(spawn) = s.agent_ops.spawn.as_mut() {
                    spawn.role = index;
                }
            },
        ))
        .child(
            SegmentedControl::new(&["Same checkout", "New managed worktree"], Some(spawn.workspace))
                .label("Workspace")
                .fit()
                .on_pick(|s: &mut AppState, index: usize, _| {
                    if let Some(spawn) = s.agent_ops.spawn.as_mut() {
                        spawn.workspace = index;
                    }
                })
                .build(k)
                .key("spawn-child-workspace"),
        )
        .child(
            TextArea::new(&spawn.prompt)
                .label("Initial prompt (optional)")
                .placeholder("What should the child do?")
                .on_edit(|s: &mut AppState| {
                    &mut s
                        .agent_ops
                        .spawn
                        .as_mut()
                        .expect("the spawn dialog is open")
                        .prompt
                })
                .build(k)
                .key("spawn-child-prompt")
                .h(110.0),
        );
    if let Some(error) = &spawn.error {
        body = body.child(
            text(error.clone())
                .key("spawn-child-error")
                .role("alert")
                .font(k.ty.text_xsmall)
                .color(k.c.danger.fg),
        );
    }
    let start_label = if spawn.starting {
        "Starting…"
    } else {
        "Spawn child"
    };
    Dialog::new("spawn-child", "Spawn child session")
        .open(true)
        .fit()
        .width(620.0)
        .on_close(|s: &mut AppState| s.agent_ops.spawn = None)
        .child(body)
        .child(footer(
            k,
            "spawn-child-close",
            |s| s.agent_ops.spawn = None,
            start_label,
            "spawn-child-start",
            start_spawn_child,
            spawn.starting || provider.is_none(),
        ))
        .build(k, layer);
}

fn send_context_dialog(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    let Some(send) = &s.agent_ops.send_context else {
        return;
    };
    let store = s.store.as_ref();
    let rows = store.map_or_else(Vec::new, |store| choices(store, true));
    let provider = selected(&rows, send.target.as_deref());
    let sessions: Vec<(SessionId, String)> = store.map_or_else(Vec::new, |store| {
        let mut out: Vec<(SessionId, String)> = store
            .sessions
            .iter()
            .filter(|session| session.id != send.source)
            .map(|session| {
                let title = s.rows.iter().find(|row| row.id == session.id).map_or_else(
                    || crate::session_label::session_title(session),
                    |row| row.title.clone(),
                );
                (session.id, title)
            })
            .collect();
        out.sort_by_key(|(id, _)| *id);
        out
    });
    let mode = if send.spawn { 1 } else { 0 };
    let mut body = col()
        .w_full()
        .gap(10.0)
        .child(
            text(format!(
                "From {}. Forge stores an envelope and either pastes it into a live session or starts a child with it as the prompt.",
                send.title
            ))
            .font(k.ty.small)
            .color(k.c.text_muted)
            .wrap(),
        )
        .child(
            SegmentedControl::new(&["Existing session", "Spawn new child"], Some(mode))
                .label("Destination")
                .fit()
                .on_pick(|s: &mut AppState, index: usize, _| {
                    if let Some(send) = s.agent_ops.send_context.as_mut() {
                        send.spawn = index == 1;
                    }
                })
                .build(k)
                .key("send-context-mode"),
        );
    if send.spawn {
        body = body
            .child(agent_picker(
                k,
                &rows,
                send.target.as_deref(),
                "send-context",
                Picker::SendContext,
            ))
            .child(role_picker(
                k,
                &CONTEXT_ROLES,
                send.role,
                "send-context-role",
                |s, index| {
                    if let Some(send) = s.agent_ops.send_context.as_mut() {
                        send.role = index;
                    }
                },
            ));
    } else if sessions.is_empty() {
        body = body.child(
            text("No other session is open to send to.")
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint),
        );
    } else {
        let mut list = col().key("send-context-sessions").gap(2.0);
        for (index, (id, title)) in sessions.iter().enumerate() {
            let chosen =
                send.target_session == Some(*id) || send.target_session.is_none() && index == 0;
            let id = *id;
            list = list.child(
                Button::new(title.clone())
                    .variant(if chosen {
                        ButtonVariant::Outline
                    } else {
                        ButtonVariant::Ghost
                    })
                    .control_size(ControlSize::Small)
                    .on_press(move |s: &mut AppState| {
                        if let Some(send) = s.agent_ops.send_context.as_mut() {
                            send.target_session = Some(id);
                        }
                    })
                    .build(k)
                    .key(format!("send-context-session-{index}")),
            );
        }
        body = body.child(
            col()
                .gap(2.0)
                .child(text("Session").font(k.ty.text_xsmall).color(k.c.text_faint))
                .child(list),
        );
    }
    body = body
        .child(
            TextInput::new(&send.summary)
                .label("Summary")
                .placeholder("One-line description of what is being handed over")
                .on_edit(|s: &mut AppState| {
                    &mut s
                        .agent_ops
                        .send_context
                        .as_mut()
                        .expect("the context dialog is open")
                        .summary
                })
                .build(k)
                .key("send-context-summary"),
        )
        .child(
            TextArea::new(&send.instructions)
                .label("Instructions")
                .placeholder("What should the receiver do with this?")
                .on_edit(|s: &mut AppState| {
                    &mut s
                        .agent_ops
                        .send_context
                        .as_mut()
                        .expect("the context dialog is open")
                        .instructions
                })
                .build(k)
                .key("send-context-instructions")
                .h(90.0),
        )
        .child(
            Checkbox::new(send.include_transcript)
                .label("Include terminal transcript")
                .on_toggle(|s: &mut AppState| {
                    if let Some(send) = s.agent_ops.send_context.as_mut() {
                        send.include_transcript = !send.include_transcript;
                    }
                })
                .build(k)
                .key("send-context-transcript"),
        );
    if let Some(error) = &send.error {
        body = body.child(
            text(error.clone())
                .key("send-context-error")
                .role("alert")
                .font(k.ty.text_xsmall)
                .color(k.c.danger.fg),
        );
    }
    let start_label = if send.starting {
        "Sending…"
    } else if send.spawn {
        "Spawn with context"
    } else {
        "Send"
    };
    let blocked = send.starting
        || (send.spawn && provider.is_none())
        || (!send.spawn && sessions.is_empty() && send.target_session.is_none());
    Dialog::new("send-context", "Send context")
        .open(true)
        .fit()
        .width(620.0)
        .on_close(|s: &mut AppState| s.agent_ops.send_context = None)
        .child(body)
        .child(footer(
            k,
            "send-context-close",
            |s| s.agent_ops.send_context = None,
            start_label,
            "send-context-start",
            start_send_context,
            blocked,
        ))
        .build(k, layer);
}

pub fn dialogs(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    handoff_dialog(s, k, layer);
    spawn_dialog(s, k, layer);
    send_context_dialog(s, k, layer);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::{Metrics, fixture_store};
    use crate::terminal::SharedTerminal;
    use std::sync::Arc;

    fn choice(key: &str, provider: &str, profile: Option<AgentProfileId>) -> Choice {
        Choice {
            key: key.into(),
            label: key.into(),
            provider: AgentProviderId::new(provider),
            profile,
        }
    }

    #[test]
    fn a_handoff_prefers_the_source_account_then_its_provider_then_the_first() {
        let profile = AgentProfileId::new();
        let rows = vec![
            choice("provider:claude", "claude", None),
            choice("profile:work", "claude", Some(profile)),
            choice("provider:opencode", "opencode", None),
        ];
        assert_eq!(
            initial_choice(&rows, Some(&AgentProviderId::new("claude")), Some(profile))
                .map(|row| row.key.as_str()),
            Some("profile:work")
        );
        assert_eq!(
            initial_choice(
                &rows,
                Some(&AgentProviderId::new("opencode")),
                Some(profile)
            )
            .map(|row| row.key.as_str()),
            Some("provider:opencode")
        );
        assert_eq!(
            initial_choice(&rows, Some(&AgentProviderId::new("gone")), None)
                .map(|row| row.key.as_str()),
            Some("provider:claude")
        );
        assert_eq!(initial_choice(&[], None, None), None);
    }

    #[test]
    fn installed_prompt_taking_profiles_are_the_handoff_choices() {
        use crate::test_support::provider;
        let store = fixture_store(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/snapshot.json"),
        )
        .expect("fixture snapshot decodes");
        let mut store = store;
        store.providers = vec![
            provider("claude", true, true, true),
            provider("opencode", true, false, false),
            provider("cursor", false, true, false),
        ];
        let rows = choices(&store, false);
        let keys: Vec<&str> = rows.iter().map(|row| row.key.as_str()).collect();
        assert_eq!(
            keys,
            ["provider:claude", "provider:opencode"],
            "an uninstalled provider is not launchable here"
        );
        let rows = choices(&store, true);
        let keys: Vec<&str> = rows.iter().map(|row| row.key.as_str()).collect();
        assert_eq!(keys, ["provider:claude"], "a launch prompt needs support");
    }

    #[test]
    fn a_captured_transcript_becomes_an_editable_prompt() {
        let store = fixture_store(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/snapshot.json"),
        )
        .expect("fixture snapshot decodes");
        let mut state =
            AppState::for_bench(store, Arc::new(Metrics::default()), SharedTerminal::new());
        let session = state.store.as_ref().expect("store").sessions[0].id;
        open_handoff(&mut state, session);
        let applied = apply_transcript(
            &mut state,
            session,
            Ok(domain::SessionTranscript {
                session_id: session,
                text: "user: fix it".into(),
                lines: 1,
                truncated: true,
            }),
        );
        assert!(applied);
        let handoff = state.agent_ops.handoff.as_ref().expect("still open");
        assert!(matches!(
            handoff.capture,
            Capture::Ready {
                truncated: true,
                ..
            }
        ));
        assert!(
            handoff
                .prompt
                .text
                .contains("(the earlier part was omitted)")
        );
        assert!(
            handoff
                .prompt
                .text
                .contains("Working directory: /home/dev/forge")
        );
        assert!(!handoff.prompt_edited);
    }

    #[test]
    fn a_finished_handoff_closes_but_a_refused_one_keeps_the_prompt() {
        let store = fixture_store(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/snapshot.json"),
        )
        .expect("fixture snapshot decodes");
        let mut state =
            AppState::for_bench(store, Arc::new(Metrics::default()), SharedTerminal::new());
        let session = state.store.as_ref().expect("store").sessions[0].id;
        open_handoff(&mut state, session);
        let mut handoff = state.agent_ops.handoff.take().expect("a handoff");
        handoff.starting = true;
        handoff.error = None;
        state.agent_ops.handoff = Some(handoff);
        command_result(&mut state, "CreateAgentSession", false, "no such provider");
        let handoff = state.agent_ops.handoff.as_ref().expect("still open");
        assert!(!handoff.starting);
        assert_eq!(handoff.error.as_deref(), Some("no such provider"));
        if let Some(handoff) = state.agent_ops.handoff.as_mut() {
            handoff.starting = true;
        }
        command_result(&mut state, "CreateAgentSession", true, "started");
        assert!(state.agent_ops.handoff.is_none());
    }

    #[test]
    fn a_context_needs_something_to_say() {
        let store = fixture_store(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/snapshot.json"),
        )
        .expect("fixture snapshot decodes");
        let mut state =
            AppState::for_bench(store, Arc::new(Metrics::default()), SharedTerminal::new());
        let session = state.store.as_ref().expect("store").sessions[0].id;
        open_send_context(&mut state, session);
        if let Some(send) = state.agent_ops.send_context.as_mut() {
            send.include_transcript = false;
        }
        start_send_context(&mut state);
        let send = state.agent_ops.send_context.as_ref().expect("still open");
        assert!(send.error.is_some());
        assert!(!send.starting);
    }
}
