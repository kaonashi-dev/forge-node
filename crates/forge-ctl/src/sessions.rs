use super::*;
use client::Store;
use domain::{ActivityState, ProjectId, Session, SessionRole, Workspace, WorkspaceKind};
use protocol::PromptDelivery;

#[derive(clap::Args)]
pub(crate) struct SpawnArgs {
    #[arg(long)]
    project: String,
    #[arg(long, conflicts_with = "worktree")]
    workspace: Option<WorkspaceId>,
    #[arg(long)]
    worktree: Option<String>,
    #[arg(long, requires = "worktree")]
    base: Option<String>,
    #[arg(long)]
    provider: String,
    #[arg(long)]
    profile: Option<String>,
    #[arg(long)]
    name: Option<String>,
    #[arg(long, conflicts_with = "prompt_file")]
    prompt: Option<String>,
    #[arg(long)]
    prompt_file: Option<PathBuf>,
    #[arg(long)]
    read_only: bool,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    effort: Option<String>,
}

pub(crate) fn dispatch(app: &App, client: &Client, cmd: SessionCmd) -> Result<(), u8> {
    match cmd {
        SessionCmd::Spawn(args) => spawn(app, client, args),
        SessionCmd::List => {
            let store = load(app, client)?;
            app.print_ok(json!({ "sessions": store.sessions.iter().map(|s| candidate(&store, s)).collect::<Vec<_>>() }));
            Ok(())
        }
        SessionCmd::Show { session } => {
            let store = load(app, client)?;
            let session = resolve(app, &store, &session)?;
            app.print_ok(serde_json::to_value(session).unwrap_or(Value::Null));
            Ok(())
        }
        SessionCmd::Read { session, max_bytes } => {
            let id = app.resolve_session(client, &session)?;
            let response = app.call(
                client,
                Request::GetSessionTranscript {
                    session_id: id,
                    max_lines: None,
                    max_bytes,
                },
                false,
            )?;
            app.print_ok(flatten(&response));
            Ok(())
        }
        SessionCmd::Kill { session } => {
            let id = app.resolve_session(client, &session)?;
            let response = app.call(client, Request::KillSession { session_id: id }, true)?;
            app.print_ok(flatten(&response));
            Ok(())
        }
        SessionCmd::Rename { session, title } => {
            let title = title.trim();
            if title.is_empty() {
                return Err(usage_code(
                    app,
                    "title cannot be empty",
                    &["forgectl", "session", "rename", "--help"],
                ));
            }
            let id = app.resolve_session(client, &session)?;
            let response = app.call(
                client,
                Request::RenameSession {
                    session_id: id,
                    title: Some(title.into()),
                },
                true,
            )?;
            app.print_ok(flatten(&response));
            Ok(())
        }
        SessionCmd::Send {
            session,
            text,
            file,
            stdin: _,
            no_submit,
            queue,
            now,
        } => {
            let text = input(app, text, file, 8_192)?;
            let id = app.resolve_session(client, &session)?;
            let response = app.call(
                client,
                Request::SendSessionPrompt {
                    caller_session_id: env_session(),
                    session_id: id,
                    text,
                    submit: !no_submit,
                    when: if now {
                        PromptDelivery::Now
                    } else if queue {
                        PromptDelivery::Queue
                    } else {
                        PromptDelivery::IfIdle
                    },
                },
                true,
            )?;
            app.print_ok(flatten(&response));
            Ok(())
        }
        SessionCmd::Wait {
            session,
            r#for,
            timeout,
        } => wait(app, client, &session, &r#for, timeout.as_deref()),
    }
}

fn load(app: &App, client: &Client) -> Result<Store, u8> {
    client.load_store().map_err(|e| app.client_error(e, false))
}

pub(crate) fn resolve<'a>(app: &App, store: &'a Store, raw: &str) -> Result<&'a Session, u8> {
    resolve_ref(store, raw).map_err(|candidates| {
        let ambiguous = !candidates.is_empty();
        app.fail(if ambiguous { 5 } else { 4 }, if ambiguous { "ambiguous" } else { "not_found" },
            if ambiguous { "session reference is ambiguous" } else { "session not found" },
            json!({ "candidates": candidates.iter().map(|s| candidate(store, s)).collect::<Vec<_>>() }),
            vec![vec!["forgectl".into(), "session".into(), "list".into()]])
    })
}

fn resolve_ref<'a>(store: &'a Store, raw: &str) -> Result<&'a Session, Vec<&'a Session>> {
    if let Ok(id) = raw.parse::<SessionId>() {
        return store
            .sessions
            .iter()
            .find(|s| s.id == id)
            .ok_or_else(Vec::new);
    }
    let prefix: Vec<_> = if raw.len() >= domain::orchestration::MIN_ID_PREFIX
        && raw.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
    {
        store
            .sessions
            .iter()
            .filter(|s| s.id.to_string().starts_with(&raw.to_ascii_lowercase()))
            .collect()
    } else {
        vec![]
    };
    if !prefix.is_empty() {
        return sole(prefix);
    }
    let (project, title) = raw
        .split_once('/')
        .map_or((None, raw), |(p, t)| (Some(p), t));
    sole(
        store
            .sessions
            .iter()
            .filter(|s| {
                s.title.resolve("session").eq_ignore_ascii_case(title)
                    && project.is_none_or(|name| {
                        project_for(store, s).is_some_and(|p| {
                            p.name.eq_ignore_ascii_case(name) || p.id.to_string() == name
                        })
                    })
            })
            .collect(),
    )
}

fn sole(matches: Vec<&Session>) -> Result<&Session, Vec<&Session>> {
    if matches.len() == 1 {
        Ok(matches[0])
    } else {
        Err(matches)
    }
}

fn project_for<'a>(store: &'a Store, session: &Session) -> Option<&'a domain::Project> {
    let workspace = store
        .workspaces
        .iter()
        .find(|w| w.id == session.workspace_id)?;
    store.projects.iter().find(|p| p.id == workspace.project_id)
}

fn candidate(store: &Store, session: &Session) -> Value {
    json!({ "id": session.id, "prefix": &session.id.to_string()[..8],
        "project": project_for(store, session).map(|p| &p.name), "workspace": session.workspace_id,
        "title": session.title.resolve("session"), "provider": session.agent_provider_id,
        "kind": format!("{:?}", session.kind), "state": format!("{:?}", session.state),
        "activity": session.activity.state })
}

fn input(app: &App, text: Option<String>, file: Option<PathBuf>, cap: usize) -> Result<String, u8> {
    let result = match (text, file) {
        (Some(text), None) if text.len() <= cap => Ok(text),
        (Some(_), None) => Err(format!("text exceeds {cap} bytes")),
        (None, Some(path)) if path != Path::new("-") => client::read_capped(&path, cap)
            .map_err(|e| e.to_string())
            .and_then(|read| {
                if read.truncated {
                    Err(format!("file exceeds {cap} bytes"))
                } else {
                    String::from_utf8(read.bytes).map_err(|e| e.to_string())
                }
            }),
        (None, _) => read_capped_stdin(cap),
        _ => Err("text and file are mutually exclusive".into()),
    };
    result.map_err(|message| usage_code(app, &message, &["forgectl", "session", "--help"]))
}

fn spawn(app: &App, client: &Client, args: SpawnArgs) -> Result<(), u8> {
    let title = args.name.as_deref().map(str::trim);
    if title == Some("") {
        return Err(usage_code(
            app,
            "name cannot be empty",
            &["forgectl", "session", "spawn", "--help"],
        ));
    }
    let prompt = if args.prompt.is_some() || args.prompt_file.is_some() {
        Some(input(app, args.prompt, args.prompt_file, 8_192)?)
    } else {
        None
    };
    let store = load(app, client)?;
    let project_path = std::fs::canonicalize(&args.project).ok();
    let projects: Vec<_> = store
        .projects
        .iter()
        .filter(|p| {
            p.id.to_string() == args.project
                || p.name.eq_ignore_ascii_case(&args.project)
                || p.root_path == Path::new(&args.project)
                || project_path.as_ref() == Some(&p.root_path)
        })
        .collect();
    let project = match projects.as_slice() {
        [p] => *p,
        [] => return Err(app.fail(4, "not_found", "project not found", json!({}), vec![])),
        _ => {
            return Err(app.fail(
                5,
                "ambiguous",
                "project is ambiguous; use its id or path",
                json!({ "projects": projects }),
                vec![],
            ))
        }
    };
    let provider = AgentProviderId::new(&args.provider);
    let profile = match args.profile.as_deref() {
        None => None,
        Some(raw) => {
            let profile = store
                .agent_profiles
                .iter()
                .find(|p| {
                    p.provider_id == provider
                        && (p.id.to_string() == raw || p.name.eq_ignore_ascii_case(raw))
                })
                .ok_or_else(|| {
                    app.fail(
                        4,
                        "not_found",
                        "profile not found for provider",
                        json!({}),
                        vec![],
                    )
                })?;
            Some(profile.id)
        }
    };
    let descriptor = store
        .providers
        .iter()
        .find(|p| p.descriptor.id == provider)
        .map(|p| &p.descriptor)
        .ok_or_else(|| app.fail(4, "not_found", "provider not found", json!({}), vec![]))?;
    if args.read_only && !descriptor.capabilities.supports_review {
        return Err(app.fail(
            1,
            "invalid_request",
            "provider has no read-only launch mode",
            json!({}),
            vec![],
        ));
    }
    if args.model.is_some() && descriptor.model.is_none()
        || args.effort.is_some() && descriptor.effort.is_none()
    {
        return Err(app.fail(
            1,
            "invalid_request",
            "provider does not support model/effort selection",
            json!({}),
            vec![],
        ));
    }
    let workspace = if let Some(branch) = &args.worktree {
        create_worktree(app, client, project.id, branch, args.base)?
    } else {
        store
            .workspaces
            .iter()
            .find(|w| {
                w.project_id == project.id
                    && args
                        .workspace
                        .map_or(w.kind == WorkspaceKind::Main, |id| w.id == id)
            })
            .cloned()
            .ok_or_else(|| {
                app.fail(
                    4,
                    "not_found",
                    "workspace not found in project",
                    json!({}),
                    vec![],
                )
            })?
    };
    let response = app.call(
        client,
        Request::CreateAgentSession {
            workspace_id: workspace.id,
            provider_id: provider.clone(),
            profile_id: profile,
            parent: None,
            role: SessionRole::Generic,
            resume: None,
            initial_prompt: prompt,
            read_only: args.read_only,
            model: args.model,
            effort: args.effort,
        },
        true,
    )?;
    let Response::SessionCreated {
        session_id,
        terminal_id,
    } = response
    else {
        return Err(app.fail(
            3,
            "unreachable",
            "unexpected spawn response",
            json!({}),
            vec![],
        ));
    };
    if let Some(title) = title {
        if let Err(code) = app.call(
            client,
            Request::RenameSession {
                session_id,
                title: Some(title.into()),
            },
            true,
        ) {
            eprintln!("session {session_id} was created but could not be renamed");
            return Err(code);
        }
    }
    app.print_ok(json!({ "session_id": session_id, "terminal_id": terminal_id, "project": project,
        "workspace": workspace, "title": title, "provider": provider, "spawned_by": env_session() }));
    Ok(())
}

fn create_worktree(
    app: &App,
    client: &Client,
    project: ProjectId,
    branch: &str,
    base: Option<String>,
) -> Result<Workspace, u8> {
    let events = client.events();
    app.call(
        client,
        Request::CreateWorktree {
            project_id: project,
            branch: branch.into(),
            base,
            name: None,
        },
        true,
    )?;
    let deadline = Instant::now() + app.timeout;
    loop {
        let event = events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|e| {
                app.fail(
                    3,
                    "unreachable",
                    &e.to_string(),
                    json!({ "uncertain": true }),
                    vec![],
                )
            })?;
        if let DaemonEvent::WorkspaceCreated(workspace) = event {
            if workspace.project_id == project && workspace.branch.as_deref() == Some(branch) {
                return Ok(workspace);
            }
        }
    }
}

fn wait(
    app: &App,
    client: &Client,
    raw: &str,
    state: &str,
    timeout: Option<&str>,
) -> Result<(), u8> {
    let wanted = match state {
        "idle" => ActivityState::Idle,
        "waiting" => ActivityState::Waiting,
        "working" => ActivityState::Working,
        _ => {
            return Err(usage_code(
                app,
                "unknown activity",
                &["forgectl", "session", "wait", "--help"],
            ))
        }
    };
    let budget = timeout
        .map(parse_duration)
        .transpose()
        .map_err(|e| usage_code(app, &e, &["forgectl", "session", "wait", "--help"]))?
        .unwrap_or(Duration::from_secs(600));
    let deadline = Instant::now() + budget;
    let events = client.events();
    let store = load(app, client)?;
    let session = resolve(app, &store, raw)?;
    let id = session.id;
    let mut current = session.clone();
    // Discard pre-snapshot events; retain updates newer than the authoritative activity clock.
    for event in events.try_iter() {
        match event {
            DaemonEvent::SessionUpdated(s)
                if s.id == id && s.activity.since >= current.activity.since =>
            {
                current = s
            }
            DaemonEvent::SessionRemoved { session_id } if session_id == id => {
                return Err(app.fail(4, "not_found", "session removed", json!({}), vec![]))
            }
            _ => {}
        }
    }
    loop {
        if current.state.is_terminal() {
            return Err(app.fail(
                5,
                "precondition_failed",
                "session ended before wait condition",
                json!({ "session_id": id, "state": current.state }),
                vec![],
            ));
        }
        if current.activity.state == wanted {
            app.print_ok(json!({ "session_id": id, "activity": current.activity }));
            return Ok(());
        }
        let event = events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|e| {
                if matches!(e, flume::RecvTimeoutError::Timeout) {
                    app.fail(
                        124,
                        "timeout",
                        "session wait timed out",
                        json!({ "session_id": id }),
                        vec![],
                    )
                } else {
                    app.fail(3, "unreachable", "daemon disconnected", json!({}), vec![])
                }
            })?;
        match event {
            DaemonEvent::SessionUpdated(s)
                if s.id == id && s.activity.since >= current.activity.since =>
            {
                current = s
            }
            DaemonEvent::SessionRemoved { session_id: s } if s == id => {
                return Err(app.fail(4, "not_found", "session removed", json!({}), vec![]))
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Store {
        let mut store = Store::new();
        let now = domain::Timestamp::now();
        for name in ["api", "web"] {
            let project = domain::Project {
                id: ProjectId::new(),
                project_group_id: None,
                name: name.into(),
                icon: None,
                root_path: format!("/dev/{name}").into(),
                git_root: None,
                created_at: now,
                last_opened_at: now,
            };
            let workspace = Workspace {
                id: WorkspaceId::new(),
                project_id: project.id,
                kind: WorkspaceKind::Main,
                path: project.root_path.clone(),
                branch: None,
                display_name: None,
                managed_by_app: false,
                created_at: now,
                status: Default::default(),
            };
            let id: SessionId = if name == "api" {
                "11111111-1111-7111-8111-111111111111"
            } else {
                "22222222-2222-7222-8222-222222222222"
            }
            .parse()
            .unwrap();
            store.sessions.push(Session {
                id,
                workspace_id: workspace.id,
                kind: domain::SessionKind::Agent,
                role: SessionRole::Generic,
                parent_session_id: None,
                root_session_id: id,
                terminal_id: None,
                editor: None,
                agent_provider_id: Some(AgentProviderId::new("claude")),
                agent_profile_id: None,
                title: domain::SessionTitle {
                    user: Some("Implement".into()),
                    ..Default::default()
                },
                state: domain::SessionState::Running,
                created_at: now,
                last_activity_at: now,
                ended_at: None,
                launch_command: None,
                base_commit: None,
                activity: domain::AgentActivity::starting(now),
                action: None,
            });
            store.projects.push(project);
            store.workspaces.push(workspace);
        }
        store
    }

    #[test]
    fn references_resolve_ids_prefixes_and_qualified_titles_or_list_ambiguity() {
        let mut store = fixture();
        let id = store.sessions[0].id;
        assert_eq!(resolve_ref(&store, &id.to_string()).unwrap().id, id);
        assert_eq!(resolve_ref(&store, &id.to_string()[..8]).unwrap().id, id);
        assert_eq!(resolve_ref(&store, "API/implement").unwrap().id, id);
        assert_eq!(resolve_ref(&store, "IMPLEMENT").unwrap_err().len(), 2);
        assert!(resolve_ref(&store, "absent").unwrap_err().is_empty());
        store.sessions[1].title.user = Some(id.to_string());
        assert_eq!(resolve_ref(&store, &id.to_string()).unwrap().id, id);
        store.sessions[1].title.user = Some("Review".into());
        assert_eq!(resolve_ref(&store, "implement").unwrap().id, id);
    }

    #[test]
    fn listing_retains_string_kind_and_state_with_exit_details() {
        let mut store = fixture();
        let running = candidate(&store, &store.sessions[0]);
        assert_eq!(running["kind"], "Agent");
        assert_eq!(running["state"], "Running");
        store.sessions[0].state = domain::SessionState::Exited {
            code: Some(0),
            signal: None,
        };
        let exited = candidate(&store, &store.sessions[0]);
        assert_eq!(exited["state"], "Exited { code: Some(0), signal: None }");
    }

    #[test]
    fn spawn_and_send_flags_enforce_exclusive_inputs() {
        assert!(Cli::try_parse_from([
            "forgectl",
            "session",
            "spawn",
            "--project",
            "api",
            "--provider",
            "claude",
            "--workspace",
            "bad-id",
            "--worktree",
            "feature"
        ])
        .is_err());
        assert!(Cli::try_parse_from([
            "forgectl", "session", "send", "worker", "--text", "hello", "--file", "file"
        ])
        .is_err());
        assert!(Cli::try_parse_from([
            "forgectl", "session", "send", "worker", "--queue", "--now", "--text", "hello"
        ])
        .is_err());
        assert!(Cli::try_parse_from(["forgectl", "session", "send", "worker", "-"]).is_ok());
        assert!(Cli::try_parse_from(["forgectl", "session", "send", "worker"]).is_err());
    }
}
