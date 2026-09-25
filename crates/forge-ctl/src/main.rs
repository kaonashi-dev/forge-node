//! `forgectl` — a client of the running Forge daemon.
//!
//! Agents inside a session and humans at a shell use the same commands.
//! The daemon keeps the ledger and enforces the rails. This process does not.

mod guide;
mod schema;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use client::{Client, ClientError};
use domain::orchestration::{parse_wait_kinds, resolve_prefix, WaitKind};
use domain::{AgentProfileId, AgentProviderId, AttemptId, RunId, SessionId, TaskId, WorkspaceId};
use protocol::{
    ClientKind, DaemonEvent, ErrorCode, IntegrateHow, ProtocolError, Request, Response,
};
use serde_json::{json, Value};

use guide::guide_text;
use schema::schema_value;

#[derive(Parser)]
#[command(
    name = "forgectl",
    about = "Drive Forge runs from a controller or a worker"
)]
struct Cli {
    /// Print `{"ok":true,"result":...}` or an error object on stdout.
    #[arg(long, global = true)]
    json: bool,
    /// Daemon socket. Defaults to `FORGE_SOCKET`, then the shared resolver.
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    /// Request budget, for example `30s` or `5m`.
    #[arg(long, global = true)]
    timeout: Option<String>,
    /// Idempotency key. A repeat returns the stored result.
    #[arg(long, global = true)]
    request_id: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Protocol version, caller identity, and orchestration limits.
    Status,
    /// Print a guide served by this binary.
    Guide { topic: Option<String> },
    /// Machine-readable command contract.
    Schema,
    /// Installed providers.
    Providers,
    #[command(subcommand)]
    Run(RunCmd),
    #[command(subcommand)]
    Task(TaskCmd),
    /// Current objective, brief, spec, accepted dependencies, and board keys.
    Context,
    /// Settle this attempt, or show a report.
    Report {
        #[arg(long, group = "outcome")]
        done: bool,
        #[arg(long, group = "outcome")]
        failed: bool,
        #[arg(long, group = "outcome")]
        blocked: bool,
        #[arg(long)]
        summary: Option<String>,
        #[arg(long)]
        verification: Option<String>,
        #[arg(long)]
        result_file: Option<PathBuf>,
        #[command(subcommand)]
        show: Option<ReportShow>,
    },
    /// Ask the controller. `--wait` blocks until the answer.
    Ask {
        text: String,
        #[arg(long)]
        wait: bool,
        #[arg(long)]
        timeout: Option<String>,
    },
    /// Read messages. `--wait` blocks until one arrives.
    Inbox {
        #[arg(long)]
        wait: bool,
        #[arg(long)]
        timeout: Option<String>,
        #[arg(long)]
        ack: Vec<String>,
        #[arg(long)]
        ack_all: bool,
        #[arg(long)]
        run: Option<String>,
    },
    /// Store a message. The body is not typed into a PTY.
    Send {
        #[arg(long)]
        to: String,
        #[arg(long, default_value = "message")]
        kind: String,
        text: Option<String>,
        #[arg(long)]
        file: Option<PathBuf>,
    },
    #[command(subcommand)]
    State(StateCmd),
    #[command(subcommand)]
    Session(SessionCmd),
    /// Provider hook. Always exits 0.
    Hook {
        #[arg(long)]
        state: String,
        #[arg(long)]
        source: Option<String>,
    },
}

#[derive(Subcommand)]
enum RunCmd {
    Start {
        #[arg(long)]
        objective: Option<String>,
        #[arg(long)]
        objective_file: Option<PathBuf>,
        /// `agent:<provider>[:profile]`, `self`, or `none`.
        #[arg(long)]
        controller: Option<String>,
        /// `new` (default), `current`, or a workspace id.
        #[arg(long, default_value = "new")]
        integration: String,
        #[arg(long)]
        base: Option<String>,
        #[arg(long)]
        project: Option<String>,
    },
    List {
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        active: bool,
    },
    Show {
        run: Option<String>,
    },
    Brief {
        run: Option<String>,
        #[arg(long)]
        set: Option<String>,
        #[arg(long)]
        set_file: Option<PathBuf>,
        #[arg(long)]
        if_version: Option<u64>,
    },
    Wait {
        run: Option<String>,
        #[arg(long)]
        r#for: String,
        #[arg(long)]
        task: Vec<String>,
        #[arg(long)]
        timeout: Option<String>,
    },
    Close {
        run: Option<String>,
        #[arg(long)]
        pr: bool,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        body_file: Option<PathBuf>,
        #[arg(long)]
        draft: bool,
        #[arg(long)]
        base: Option<String>,
        #[arg(long)]
        cancel: bool,
        #[arg(long)]
        cleanup: bool,
    },
    Resume {
        run: String,
        #[arg(long)]
        provider: String,
        #[arg(long)]
        profile: Option<String>,
    },
}

#[derive(Subcommand)]
enum TaskCmd {
    Add {
        #[arg(long)]
        title: String,
        #[arg(long)]
        spec: Option<String>,
        #[arg(long)]
        spec_file: Option<PathBuf>,
        #[arg(long)]
        acceptance: Option<String>,
        #[arg(long)]
        after: Vec<String>,
        #[arg(long)]
        read_only: bool,
        #[arg(long)]
        allow_subruns: bool,
        #[arg(long)]
        run: Option<String>,
    },
    List {
        #[arg(long)]
        run: Option<String>,
        #[arg(long)]
        ready: bool,
    },
    Show {
        task: String,
    },
    Start {
        task: String,
        #[arg(long)]
        provider: String,
        #[arg(long)]
        profile: Option<String>,
        /// `worktree` (default for write), `integration`, `same`, or a workspace id.
        #[arg(long)]
        placement: Option<String>,
        #[arg(long)]
        branch: Option<String>,
    },
    Review {
        task: String,
        #[arg(long)]
        patch: bool,
    },
    Accept {
        task: String,
        #[arg(long)]
        note: Option<String>,
    },
    Reject {
        task: String,
        #[arg(long)]
        feedback: String,
        #[arg(long)]
        retry: bool,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        profile: Option<String>,
    },
    Cancel {
        task: String,
        #[arg(long)]
        kill: bool,
    },
    Integrate {
        task: String,
        #[arg(long)]
        merge: bool,
        #[arg(long)]
        squash: bool,
        #[arg(long)]
        r#continue: bool,
        #[arg(long)]
        abort: bool,
    },
    Cleanup {
        task: String,
        #[arg(long)]
        keep_worktree: bool,
    },
}

#[derive(Subcommand)]
enum ReportShow {
    Show {
        task: String,
        #[arg(long)]
        attempt: Option<u32>,
    },
}

#[derive(Subcommand)]
enum StateCmd {
    List {
        #[arg(long)]
        prefix: Option<String>,
    },
    Get {
        key: String,
    },
    Set {
        key: String,
        value: String,
        #[arg(long)]
        if_version: u64,
    },
    Del {
        key: String,
        #[arg(long)]
        if_version: u64,
    },
    Watch {
        key: String,
        #[arg(long)]
        from_version: Option<u64>,
        #[arg(long)]
        timeout: Option<String>,
    },
}

#[derive(Subcommand)]
enum SessionCmd {
    List,
    Show {
        session: String,
    },
    Read {
        session: String,
        #[arg(long)]
        max_bytes: Option<u32>,
    },
    Kill {
        session: String,
    },
}

struct App {
    json: bool,
    socket: Option<PathBuf>,
    timeout: Duration,
    request_id: Option<String>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let app = App {
        json: cli.json || std::env::var("FORGECTL_JSON").ok().as_deref() == Some("1"),
        socket: cli.socket,
        timeout: cli
            .timeout
            .as_deref()
            .map(parse_duration)
            .transpose()
            .unwrap_or_else(|message| usage_exit(&message, &["forgectl", "--help"]))
            .unwrap_or(Duration::from_secs(30)),
        request_id: cli.request_id,
    };
    match app.execute(cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => ExitCode::from(code),
    }
}

impl App {
    fn execute(&self, command: Command) -> Result<(), u8> {
        match command {
            Command::Guide { topic } => {
                let topic = topic.unwrap_or_else(|| "controller".into());
                let text = guide_text(&topic).ok_or_else(|| {
                    usage_code(
                        self,
                        &format!("unknown guide {topic}"),
                        &["forgectl", "guide"],
                    )
                })?;
                if self.json {
                    self.print_ok(json!({ "topic": topic, "text": text }));
                } else {
                    println!("{text}");
                }
                Ok(())
            }
            Command::Schema => {
                self.print_ok(schema_value());
                Ok(())
            }
            Command::Hook { state, source } => {
                let _ = self.hook(&state, source.as_deref());
                Ok(())
            }
            other => self.with_client(|client| self.dispatch(client, other)),
        }
    }

    fn dispatch(&self, client: &Client, command: Command) -> Result<(), u8> {
        match command {
            Command::Status => {
                let status = self.call(client, Request::OrchestrationStatus, false)?;
                let info = client.daemon_info();
                let limits = match status {
                    Response::OrchestrationStatus(limits) => limits,
                    other => {
                        return Err(self.fail(
                            1,
                            "internal",
                            "status was not understood",
                            json!({ "got": format!("{other:?}") }),
                            vec![],
                        ))
                    }
                };
                self.print_ok(json!({
                    "protocol_version": info.protocol_version,
                    "daemon_version": info.daemon_version,
                    "caller": self.caller_json(),
                    "limits": limits,
                }));
                Ok(())
            }
            Command::Providers => {
                let response = self.call(client, Request::ListAgentProviders, false)?;
                self.print_ok(flatten(&response));
                Ok(())
            }
            Command::Run(cmd) => self.run_cmd(client, cmd),
            Command::Task(cmd) => self.task_cmd(client, cmd),
            Command::Context => self.context_cmd(client),
            Command::Report {
                done,
                failed,
                blocked,
                summary,
                verification,
                result_file,
                show,
            } => {
                if let Some(ReportShow::Show { task, attempt }) = show {
                    return self.report_show(client, &task, attempt);
                }
                let outcome = if done {
                    "done"
                } else if failed {
                    "failed"
                } else if blocked {
                    "blocked"
                } else {
                    return Err(usage_code(
                        self,
                        "report needs --done, --failed, or --blocked",
                        &["forgectl", "report", "--help"],
                    ));
                };
                let summary = summary.ok_or_else(|| {
                    usage_code(
                        self,
                        "report needs --summary",
                        &["forgectl", "report", "--help"],
                    )
                })?;
                self.report_submit(client, outcome, summary, verification, result_file)
            }
            Command::Ask {
                text,
                wait,
                timeout,
            } => self.ask(client, text, wait, timeout),
            Command::Inbox {
                wait,
                timeout,
                ack,
                ack_all,
                run,
            } => self.inbox(client, wait, timeout, ack, ack_all, run),
            Command::Send {
                to,
                kind,
                text,
                file,
            } => self.send(client, to, kind, text, file),
            Command::State(cmd) => self.state_cmd(client, cmd),
            Command::Session(cmd) => self.session_cmd(client, cmd),
            Command::Guide { .. } | Command::Schema | Command::Hook { .. } => Ok(()),
        }
    }

    fn run_cmd(&self, client: &Client, cmd: RunCmd) -> Result<(), u8> {
        match cmd {
            RunCmd::Start {
                objective,
                objective_file,
                controller,
                integration,
                base,
                project,
            } => {
                let objective = read_text(objective, objective_file).map_err(|message| {
                    usage_code(self, &message, &["forgectl", "run", "start", "--help"])
                })?;
                let project_id = self.resolve_project(client, project.as_deref())?;
                let controller = parse_controller(controller.as_deref(), env_session().is_some())
                    .map_err(|message| {
                    usage_code(self, &message, &["forgectl", "run", "start", "--help"])
                })?;
                let integration = parse_integration(&integration)?;
                let response = self.call(
                    client,
                    Request::CreateRun {
                        request_id: self.request_id.clone(),
                        caller_session_id: env_session(),
                        project_id,
                        objective,
                        controller,
                        integration,
                        base,
                        parent_attempt_id: None,
                    },
                    true,
                )?;
                self.print_ok(flatten(&response));
                Ok(())
            }
            RunCmd::List { project, active } => {
                let project_id = match project {
                    Some(project) => Some(self.resolve_project(client, Some(&project))?),
                    None => None,
                };
                let response = self.call(
                    client,
                    Request::ListRuns {
                        project_id,
                        active_only: active,
                    },
                    false,
                )?;
                self.print_ok(flatten(&response));
                Ok(())
            }
            RunCmd::Show { run } => {
                let run_id = self.resolve_run(client, run.as_deref())?;
                let response = self.call(client, Request::GetRun { run_id }, false)?;
                self.print_ok(flatten(&response));
                Ok(())
            }
            RunCmd::Brief {
                run,
                set,
                set_file,
                if_version,
            } => {
                let run_id = self.resolve_run(client, run.as_deref())?;
                let brief = read_text(set, set_file).map_err(|message| {
                    usage_code(self, &message, &["forgectl", "run", "brief", "--help"])
                })?;
                let expected_version = if_version.ok_or_else(|| {
                    usage_code(
                        self,
                        "run brief needs --if-version",
                        &["forgectl", "run", "brief", "--help"],
                    )
                })?;
                let response = self.call(
                    client,
                    Request::UpdateRunBrief {
                        request_id: self.request_id.clone(),
                        caller_session_id: env_session(),
                        run_id,
                        brief,
                        expected_version,
                    },
                    true,
                )?;
                self.print_ok(flatten(&response));
                Ok(())
            }
            RunCmd::Wait {
                run,
                r#for,
                task,
                timeout,
            } => self.run_wait(client, run, r#for, task, timeout),
            RunCmd::Close {
                run,
                pr,
                title,
                body_file,
                draft,
                base,
                cancel,
                cleanup,
            } => {
                let run_id = self.resolve_run(client, run.as_deref())?;
                let pull_request = if pr {
                    let title = title.unwrap_or_else(|| "Forge run".into());
                    let body = body_file
                        .map(|path| read_capped_text(&path, 64 * 1024))
                        .transpose()?
                        .unwrap_or_default();
                    Some(protocol::CloseRunPullRequest {
                        title,
                        body,
                        draft,
                        base,
                    })
                } else {
                    None
                };
                let response = self.call(
                    client,
                    Request::CloseRun {
                        request_id: self.request_id.clone(),
                        caller_session_id: env_session(),
                        run_id,
                        cancel,
                        cleanup,
                        pull_request,
                    },
                    true,
                )?;
                let mut value = flatten(&response);
                if pr {
                    let opened = wait_pull_request(client, Duration::from_secs(30));
                    match opened {
                        Some(DaemonEvent::PullRequestOpened { url: Some(url), .. }) => {
                            if let Some(object) = value.as_object_mut() {
                                object.insert("pull_request".into(), json!({ "url": url }));
                            }
                        }
                        Some(DaemonEvent::PullRequestOpened {
                            error: Some(error), ..
                        }) => {
                            return Err(self.fail(
                                1,
                                "git_error",
                                &error,
                                json!({ "residual": value.get("residual").cloned().unwrap_or(json!([])) }),
                                vec![vec!["forgectl".into(), "run".into(), "show".into()]],
                            ));
                        }
                        _ => {
                            return Err(self.fail(
                                1,
                                "git_error",
                                "pull request did not open",
                                json!({}),
                                vec![vec!["forgectl".into(), "run".into(), "show".into()]],
                            ));
                        }
                    }
                }
                self.print_ok(value);
                Ok(())
            }
            RunCmd::Resume {
                run,
                provider,
                profile,
            } => {
                let run_id = self.resolve_run(client, Some(&run))?;
                let response = self.call(
                    client,
                    Request::ResumeRun {
                        request_id: self.request_id.clone(),
                        caller_session_id: env_session(),
                        run_id,
                        provider_id: AgentProviderId::new(provider),
                        profile_id: optional_profile(profile.as_deref()).map_err(|message| {
                            usage_code(self, &message, &["forgectl", "run", "resume", "--help"])
                        })?,
                    },
                    true,
                )?;
                self.print_ok(flatten(&response));
                Ok(())
            }
        }
    }

    fn task_cmd(&self, client: &Client, cmd: TaskCmd) -> Result<(), u8> {
        match cmd {
            TaskCmd::Add {
                title,
                spec,
                spec_file,
                acceptance,
                after,
                read_only,
                allow_subruns,
                run,
            } => {
                let run_id = self.resolve_run(client, run.as_deref())?;
                let spec = read_text(spec, spec_file).unwrap_or_default();
                let after = self.resolve_tasks(client, run_id, &after)?;
                let response = self.call(
                    client,
                    Request::CreateTask {
                        request_id: self.request_id.clone(),
                        caller_session_id: env_session(),
                        run_id,
                        title,
                        spec,
                        acceptance: acceptance.unwrap_or_default(),
                        after,
                        mode: if read_only {
                            domain::orchestration::WorkMode::ReadOnly
                        } else {
                            domain::orchestration::WorkMode::Write
                        },
                        allow_subruns,
                    },
                    true,
                )?;
                self.print_ok(flatten(&response));
                Ok(())
            }
            TaskCmd::List { run, ready } => {
                let run_id = self.resolve_run(client, run.as_deref())?;
                let response = self.call(client, Request::GetRun { run_id }, false)?;
                let mut value = flatten(&response);
                if ready {
                    if let Some(tasks) = value.get_mut("tasks").and_then(|v| v.as_array_mut()) {
                        tasks.retain(|task| {
                            task.get("status").and_then(|s| s.as_str()) == Some("ready")
                        });
                    }
                }
                self.print_ok(value);
                Ok(())
            }
            TaskCmd::Show { task } => {
                let (run_id, task_id) = self.find_task(client, &task)?;
                let response = self.call(client, Request::GetRun { run_id }, false)?;
                self.print_ok(task_slice(&flatten(&response), task_id));
                Ok(())
            }
            TaskCmd::Start {
                task,
                provider,
                profile,
                placement,
                branch,
            } => {
                let (_run, task_id) = self.find_task(client, &task)?;
                let response = self.call(
                    client,
                    Request::StartAttempt {
                        request_id: self.request_id.clone(),
                        caller_session_id: env_session(),
                        task_id,
                        provider_id: AgentProviderId::new(provider),
                        profile_id: optional_profile(profile.as_deref()).map_err(|message| {
                            usage_code(self, &message, &["forgectl", "task", "start", "--help"])
                        })?,
                        placement: parse_placement(placement.as_deref())?,
                        branch,
                    },
                    true,
                )?;
                self.print_ok(flatten(&response));
                Ok(())
            }
            TaskCmd::Review { task, patch } => {
                let (_run, task_id) = self.find_task(client, &task)?;
                let response = self.call(client, Request::ReviewTask { task_id, patch }, false)?;
                self.print_ok(flatten(&response));
                Ok(())
            }
            TaskCmd::Accept { task, note } => self.decide(
                client,
                &task,
                domain::orchestration::TaskDecision::Accept { note },
                None,
                None,
            ),
            TaskCmd::Reject {
                task,
                feedback,
                retry,
                provider,
                profile,
            } => self.decide(
                client,
                &task,
                domain::orchestration::TaskDecision::Reject { feedback, retry },
                provider,
                profile,
            ),
            TaskCmd::Cancel { task, kill } => self.decide(
                client,
                &task,
                domain::orchestration::TaskDecision::Cancel { kill },
                None,
                None,
            ),
            TaskCmd::Integrate {
                task,
                merge: _,
                squash,
                r#continue,
                abort,
            } => {
                let (_run, task_id) = self.find_task(client, &task)?;
                let how = if r#continue {
                    IntegrateHow::Continue
                } else if abort {
                    IntegrateHow::Abort
                } else {
                    IntegrateHow::Merge { squash }
                };
                let response = self.call(
                    client,
                    Request::IntegrateTask {
                        request_id: self.request_id.clone(),
                        caller_session_id: env_session(),
                        task_id,
                        how,
                    },
                    true,
                )?;
                self.print_ok(flatten(&response));
                Ok(())
            }
            TaskCmd::Cleanup {
                task,
                keep_worktree,
            } => {
                let (_run, task_id) = self.find_task(client, &task)?;
                let response = self.call(
                    client,
                    Request::CleanupTask {
                        request_id: self.request_id.clone(),
                        caller_session_id: env_session(),
                        task_id,
                        keep_worktree,
                    },
                    true,
                )?;
                self.print_ok(flatten(&response));
                Ok(())
            }
        }
    }

    fn decide(
        &self,
        client: &Client,
        task: &str,
        decision: domain::orchestration::TaskDecision,
        provider: Option<String>,
        profile: Option<String>,
    ) -> Result<(), u8> {
        let (run_id, task_id) = self.find_task(client, task)?;
        let view = self.call(client, Request::GetRun { run_id }, false)?;
        let revision = flatten(&view)
            .get("tasks")
            .and_then(|tasks| tasks.as_array())
            .and_then(|tasks| {
                tasks.iter().find(|task| {
                    task.get("id").and_then(|id| id.as_str()) == Some(&task_id.to_string())
                })
            })
            .and_then(|task| task.get("revision").and_then(|v| v.as_u64()))
            .unwrap_or(1);
        let response = self.call(
            client,
            Request::DecideTask {
                request_id: self.request_id.clone(),
                caller_session_id: env_session(),
                task_id,
                expected_revision: revision,
                decision,
                retry_provider_id: provider.map(AgentProviderId::new),
                retry_profile_id: optional_profile(profile.as_deref()).map_err(|message| {
                    usage_code(self, &message, &["forgectl", "task", "accept", "--help"])
                })?,
            },
            true,
        )?;
        self.print_ok(flatten(&response));
        Ok(())
    }

    fn context_cmd(&self, client: &Client) -> Result<(), u8> {
        let run_id = self.resolve_run(
            client,
            env_run().as_ref().map(|id| id.to_string()).as_deref(),
        )?;
        let view = flatten(&self.call(client, Request::GetRun { run_id }, false)?);
        let board = flatten(&self.call(
            client,
            Request::ListRunState {
                run_id,
                prefix: None,
            },
            false,
        )?);
        let task_id = env_task();
        let task = view
            .get("tasks")
            .and_then(|v| v.as_array())
            .and_then(|tasks| {
                tasks.iter().find(|task| {
                    task_id
                        .map(|id| task.get("id").and_then(|v| v.as_str()) == Some(&id.to_string()))
                        .unwrap_or(true)
                })
            });
        let parsed: domain::orchestration::RunView =
            serde_json::from_value(view.clone()).map_err(|error| {
                self.fail(
                    1,
                    "internal",
                    &error.to_string(),
                    json!({}),
                    vec![vec!["forgectl".into(), "context".into()]],
                )
            })?;
        let entries: Vec<domain::orchestration::BoardEntry> =
            serde_json::from_value(board).unwrap_or_default();
        let selected = parsed
            .tasks
            .iter()
            .find(|candidate| task_id.map(|id| candidate.id == id).unwrap_or(true))
            .map(|candidate| {
                (
                    candidate.after.clone(),
                    candidate.spec.clone(),
                    candidate.acceptance.clone(),
                )
            });
        let (after, spec, acceptance) = selected.unwrap_or_default();
        let summaries = domain::orchestration::accepted_dependency_summaries(
            &after,
            &parsed.tasks,
            &parsed.attempts,
        );
        let board_owned: Vec<(String, Option<String>)> = entries
            .iter()
            .map(|entry| (entry.key.clone(), Some(entry.value_json.clone())))
            .collect();
        let board_refs: Vec<(&str, Option<&str>)> = board_owned
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_deref()))
            .collect();
        let unread = if let Some(session) = env_session() {
            let inbox = flatten(&self.call(
                client,
                Request::ReadInbox {
                    session_id: Some(session),
                    run_id: Some(run_id),
                    unread_only: true,
                    limit: domain::orchestration::INBOX_PAGE as u32,
                    ack: vec![],
                },
                false,
            )?);
            inbox
                .get("messages")
                .and_then(|messages| messages.as_array())
                .map(|messages| messages.len())
                .unwrap_or(0)
        } else {
            0
        };
        let digest = domain::orchestration::compose_context_digest(
            &domain::orchestration::ContextDigestInput {
                objective: &parsed.run.objective,
                brief: &parsed.run.brief,
                brief_version: parsed.run.brief_version,
                spec: &spec,
                acceptance: &acceptance,
                dependencies: &summaries,
                board: &board_refs,
                unread,
            },
        );
        self.print_ok(json!({
            "digest": digest,
            "run": view.get("run").cloned().unwrap_or(Value::Null),
            "task": task.cloned().unwrap_or(Value::Null),
        }));
        Ok(())
    }

    fn report_submit(
        &self,
        client: &Client,
        outcome: &str,
        summary: String,
        verification: Option<String>,
        result_file: Option<PathBuf>,
    ) -> Result<(), u8> {
        let attempt_id = env_attempt().ok_or_else(|| {
            usage_code(
                self,
                "FORGE_ATTEMPT_ID is not set",
                &["forgectl", "report", "--help"],
            )
        })?;
        let outcome = match outcome {
            "done" => domain::orchestration::ReportOutcome::Done,
            "failed" => domain::orchestration::ReportOutcome::Failed,
            _ => domain::orchestration::ReportOutcome::Blocked,
        };
        let response = self.call(
            client,
            Request::ReportAttempt {
                request_id: self.request_id.clone(),
                caller_session_id: env_session(),
                attempt_id,
                outcome,
                summary,
                verification,
                result_file: result_file.map(|path| path.to_string_lossy().into_owned()),
            },
            true,
        )?;
        self.print_ok(flatten(&response));
        Ok(())
    }

    fn report_show(&self, client: &Client, task: &str, attempt_n: Option<u32>) -> Result<(), u8> {
        let (run_id, task_id) = self.find_task(client, task)?;
        let view = flatten(&self.call(client, Request::GetRun { run_id }, false)?);
        let attempt = view
            .get("attempts")
            .and_then(|v| v.as_array())
            .and_then(|attempts| {
                attempts
                    .iter()
                    .filter(|attempt| {
                        attempt.get("task_id").and_then(|v| v.as_str())
                            == Some(&task_id.to_string())
                    })
                    .filter(|attempt| {
                        attempt_n
                            .map(|n| {
                                attempt.get("n").and_then(|v| v.as_u64()) == Some(u64::from(n))
                            })
                            .unwrap_or(true)
                    })
                    .max_by_key(|attempt| attempt.get("n").and_then(|v| v.as_u64()).unwrap_or(0))
                    .cloned()
            });
        self.print_ok(json!({ "task_id": task_id, "attempt": attempt }));
        Ok(())
    }

    fn ask(
        &self,
        client: &Client,
        text: String,
        wait: bool,
        timeout: Option<String>,
    ) -> Result<(), u8> {
        let run_id = self.resolve_run(client, None)?;
        if wait {
            if let Some(session) = env_session() {
                let _ = client.request(Request::WatchInbox {
                    session_id: Some(session),
                    run_id: Some(run_id),
                });
            }
        }
        let posted = self.call(
            client,
            Request::PostMessage {
                request_id: self.request_id.clone(),
                caller_session_id: env_session(),
                run_id,
                address: "controller".into(),
                kind: domain::ContextKind::Question,
                body: text,
                in_reply_to: None,
            },
            true,
        )?;
        if !wait {
            self.print_ok(flatten(&posted));
            return Ok(());
        }
        let id = flatten(&posted)
            .get("ids")
            .and_then(|v| v.as_array())
            .and_then(|ids| ids.first())
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok());
        let budget = timeout
            .as_deref()
            .map(parse_duration)
            .transpose()
            .unwrap_or(None)
            .unwrap_or(self.timeout);
        let deadline = Instant::now() + budget;
        loop {
            if Instant::now() >= deadline {
                return Err(self.fail(
                    124,
                    "timeout",
                    "wait timed out",
                    json!({}),
                    vec![vec!["forgectl".into(), "inbox".into()]],
                ));
            }
            let inbox = self.call(
                client,
                Request::ReadInbox {
                    session_id: env_session(),
                    run_id: Some(run_id),
                    unread_only: false,
                    limit: 50,
                    ack: vec![],
                },
                false,
            )?;
            if let Some(answer) = flatten(&inbox)
                .get("messages")
                .and_then(|v| v.as_array())
                .and_then(|messages| {
                    messages.iter().find(|message| {
                        message.get("kind").and_then(|v| v.as_str()) == Some("answer")
                            && message.get("in_reply_to").and_then(|v| v.as_str())
                                == id
                                    .as_ref()
                                    .map(|id: &domain::ContextId| id.to_string())
                                    .as_deref()
                    })
                })
                .cloned()
            {
                self.print_ok(answer);
                return Ok(());
            }
            let _ = client.events().recv_timeout(Duration::from_millis(200));
        }
    }

    fn inbox(
        &self,
        client: &Client,
        wait: bool,
        timeout: Option<String>,
        ack: Vec<String>,
        ack_all: bool,
        run: Option<String>,
    ) -> Result<(), u8> {
        let run_id = run
            .as_deref()
            .map(|run| self.resolve_run(client, Some(run)))
            .transpose()?;
        let session = env_session();
        if wait {
            let _ = client.request(Request::WatchInbox {
                session_id: session,
                run_id,
            });
        }
        let ack_ids = ack
            .iter()
            .filter_map(|id| id.parse().ok())
            .collect::<Vec<domain::ContextId>>();
        let read = |ack: Vec<domain::ContextId>| {
            self.call(
                client,
                Request::ReadInbox {
                    session_id: session,
                    run_id,
                    unread_only: false,
                    limit: 50,
                    ack,
                },
                false,
            )
        };
        if !wait {
            let response = read(ack_ids)?;
            self.print_ok(flatten(&response));
            return Ok(());
        }
        let budget = timeout
            .as_deref()
            .map(parse_duration)
            .transpose()
            .unwrap_or(None)
            .unwrap_or(self.timeout);
        let deadline = Instant::now() + budget;
        loop {
            let response = read(if ack_all { vec![] } else { ack_ids.clone() })?;
            let value = flatten(&response);
            let empty = value
                .get("messages")
                .and_then(|v| v.as_array())
                .is_none_or(|messages| messages.is_empty());
            if !empty {
                self.print_ok(value);
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(self.fail(
                    124,
                    "timeout",
                    "wait timed out",
                    json!({}),
                    vec![vec!["forgectl".into(), "inbox".into()]],
                ));
            }
            let _ = client.events().recv_timeout(Duration::from_millis(200));
        }
    }

    fn send(
        &self,
        client: &Client,
        to: String,
        kind: String,
        text: Option<String>,
        file: Option<PathBuf>,
    ) -> Result<(), u8> {
        let run_id = self.resolve_run(client, None)?;
        let body = read_text(text, file)
            .map_err(|message| usage_code(self, &message, &["forgectl", "send", "--help"]))?;
        let kind = match kind.as_str() {
            "answer" => domain::ContextKind::Answer,
            "feedback" => domain::ContextKind::Feedback,
            "question" => domain::ContextKind::Question,
            _ => domain::ContextKind::Message,
        };
        let response = self.call(
            client,
            Request::PostMessage {
                request_id: self.request_id.clone(),
                caller_session_id: env_session(),
                run_id,
                address: to,
                kind,
                body,
                in_reply_to: None,
            },
            true,
        )?;
        self.print_ok(flatten(&response));
        Ok(())
    }

    fn state_cmd(&self, client: &Client, cmd: StateCmd) -> Result<(), u8> {
        let run_id = self.resolve_run(client, None)?;
        match cmd {
            StateCmd::List { prefix } => {
                let response =
                    self.call(client, Request::ListRunState { run_id, prefix }, false)?;
                self.print_ok(flatten(&response));
            }
            StateCmd::Get { key } => {
                let response = self.call(client, Request::GetRunState { run_id, key }, false)?;
                self.print_ok(flatten(&response));
            }
            StateCmd::Set {
                key,
                value,
                if_version,
            } => {
                let response = self.call(
                    client,
                    Request::SetRunState {
                        request_id: self.request_id.clone(),
                        caller_session_id: env_session(),
                        run_id,
                        key,
                        value_json: value,
                        expected_version: if_version,
                    },
                    true,
                )?;
                self.print_ok(flatten(&response));
            }
            StateCmd::Del { key, if_version } => {
                let response = self.call(
                    client,
                    Request::DeleteRunState {
                        request_id: self.request_id.clone(),
                        caller_session_id: env_session(),
                        run_id,
                        key,
                        expected_version: if_version,
                    },
                    true,
                )?;
                self.print_ok(flatten(&response));
            }
            StateCmd::Watch {
                key,
                from_version,
                timeout,
            } => {
                let budget = timeout
                    .as_deref()
                    .map(parse_duration)
                    .transpose()
                    .unwrap_or(None)
                    .unwrap_or(self.timeout);
                let current = self.call(
                    client,
                    Request::GetRunState {
                        run_id,
                        key: key.clone(),
                    },
                    false,
                );
                if let Ok(response) = &current {
                    let version = flatten(response)
                        .get("version")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    if from_version.is_none_or(|from| version > from) {
                        self.print_ok(flatten(response));
                        return Ok(());
                    }
                }
                let deadline = Instant::now() + budget;
                loop {
                    if Instant::now() >= deadline {
                        return Err(self.fail(
                            124,
                            "timeout",
                            "wait timed out",
                            json!({}),
                            vec![vec!["forgectl".into(), "state".into(), "get".into(), key]],
                        ));
                    }
                    if let Ok(DaemonEvent::RunStateChanged {
                        run_id: event_run,
                        key: event_key,
                        ..
                    }) = client.events().recv_timeout(Duration::from_millis(200))
                    {
                        if event_run == run_id && event_key == key {
                            let response = self.call(
                                client,
                                Request::GetRunState {
                                    run_id,
                                    key: key.clone(),
                                },
                                false,
                            )?;
                            self.print_ok(flatten(&response));
                            return Ok(());
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn session_cmd(&self, client: &Client, cmd: SessionCmd) -> Result<(), u8> {
        match cmd {
            SessionCmd::List => {
                let store = client
                    .load_store()
                    .map_err(|error| self.client_error(error, false))?;
                self.print_ok(json!({
                    "sessions": store.sessions.iter().map(|session| json!({
                        "id": session.id,
                        "state": format!("{:?}", session.state),
                        "kind": format!("{:?}", session.kind),
                        "activity": session.activity.state,
                        "title": session.title.resolve("session"),
                    })).collect::<Vec<_>>(),
                }));
            }
            SessionCmd::Show { session } => {
                let id = self.resolve_session(client, &session)?;
                let store = client
                    .load_store()
                    .map_err(|error| self.client_error(error, false))?;
                let found = store.sessions.into_iter().find(|session| session.id == id);
                match found {
                    Some(session) => {
                        self.print_ok(serde_json::to_value(session).unwrap_or(Value::Null))
                    }
                    None => {
                        return Err(self.fail(
                            4,
                            "not_found",
                            "session not found",
                            json!({}),
                            vec![vec!["forgectl".into(), "session".into(), "list".into()]],
                        ))
                    }
                }
            }
            SessionCmd::Read { session, max_bytes } => {
                let id = self.resolve_session(client, &session)?;
                let response = self.call(
                    client,
                    Request::GetSessionTranscript {
                        session_id: id,
                        max_lines: None,
                        max_bytes,
                    },
                    false,
                )?;
                self.print_ok(flatten(&response));
            }
            SessionCmd::Kill { session } => {
                let id = self.resolve_session(client, &session)?;
                let response = self.call(client, Request::KillSession { session_id: id }, true)?;
                self.print_ok(flatten(&response));
            }
        }
        Ok(())
    }

    fn run_wait(
        &self,
        client: &Client,
        run: Option<String>,
        kinds_raw: String,
        tasks: Vec<String>,
        timeout: Option<String>,
    ) -> Result<(), u8> {
        let kinds = parse_wait_kinds(&kinds_raw).map_err(|_| {
            usage_code(
                self,
                "wait --for needs attention, reported, question, or settled",
                &["forgectl", "run", "wait", "--help"],
            )
        })?;
        let run_id = self.resolve_run(client, run.as_deref())?;
        let task_ids = self.resolve_tasks(client, run_id, &tasks)?;
        let budget = timeout
            .as_deref()
            .map(parse_duration)
            .transpose()
            .unwrap_or(None)
            .unwrap_or(self.timeout);
        let deadline = Instant::now() + budget;
        loop {
            let view = flatten(&self.call(client, Request::GetRun { run_id }, false)?);
            let triggered = wake_items(&view, &kinds, &task_ids);
            if !triggered.is_empty() {
                self.print_ok(json!({
                    "run_id": run_id,
                    "triggered": triggered,
                }));
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(self.fail(
                    124,
                    "timeout",
                    "wait timed out",
                    json!({ "run_id": run_id }),
                    vec![vec![
                        "forgectl".into(),
                        "run".into(),
                        "show".into(),
                        run_id.to_string(),
                    ]],
                ));
            }
            let _ = client.events().recv_timeout(Duration::from_millis(200));
        }
    }

    fn hook(&self, state: &str, source: Option<&str>) -> Result<(), u8> {
        let Some(session) = env_session() else {
            return Ok(());
        };
        let state = match state {
            "working" => domain::ActivityState::Working,
            "waiting" => domain::ActivityState::Waiting,
            "idle" => domain::ActivityState::Idle,
            _ => return Ok(()),
        };
        let Ok(client) = self.connect() else {
            return Ok(());
        };
        let _ = client.request_timeout(
            Request::ReportAgentActivity {
                session_id: session,
                state,
                source: source.unwrap_or("hook").into(),
            },
            Duration::from_secs(1),
        );
        Ok(())
    }

    fn with_client(&self, body: impl FnOnce(&Client) -> Result<(), u8>) -> Result<(), u8> {
        let client = self.connect()?;
        body(&client)
    }

    fn connect(&self) -> Result<Client, u8> {
        let socket = self
            .socket
            .clone()
            .or_else(|| std::env::var_os("FORGE_SOCKET").map(PathBuf::from));
        let socket = match socket {
            Some(path) => path,
            None => client::socket_path().map_err(|error| {
                self.fail(
                    3,
                    "unreachable",
                    &error,
                    json!({}),
                    vec![vec!["forgectl".into(), "status".into()]],
                )
            })?,
        };
        match Client::connect_as(&socket, env!("CARGO_PKG_VERSION"), ClientKind::Cli) {
            Ok(client) => Ok(client),
            Err(ClientError::VersionMismatch { expected, got }) => Err(self.fail(
                3,
                "unreachable",
                &format!("protocol {expected} != {got}"),
                json!({ "expected": expected, "got": got }),
                vec![vec!["forgectl".into(), "status".into()]],
            )),
            Err(ClientError::Io(error)) if denied(&error) => Err(self.fail(
                3,
                "sandbox_denied",
                &error.to_string(),
                json!({}),
                vec![vec!["forgectl".into(), "status".into()]],
            )),
            Err(error) => Err(self.fail(
                3,
                "unreachable",
                &error.to_string(),
                json!({}),
                vec![vec!["forgectl".into(), "status".into()]],
            )),
        }
    }

    fn call(&self, client: &Client, request: Request, mutation: bool) -> Result<Response, u8> {
        match client.request_timeout(request, self.timeout) {
            Ok(response) => Ok(response),
            Err(ClientError::Protocol(error)) => Err(self.protocol_error(&error)),
            Err(ClientError::Timeout | ClientError::Disconnected) if mutation => Err(self.fail(
                3,
                "unreachable",
                "the outcome is uncertain; retry with the same --request-id",
                json!({ "uncertain": true }),
                vec![vec!["forgectl".into(), "status".into()]],
            )),
            Err(error) => Err(self.client_error(error, mutation)),
        }
    }

    fn protocol_error(&self, error: &ProtocolError) -> u8 {
        let details = error
            .details
            .as_deref()
            .and_then(|text| serde_json::from_str::<Value>(text).ok())
            .unwrap_or_else(|| json!({ "text": error.details.clone().unwrap_or_default() }));
        let reason = details
            .get("reason")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let (code, name, next) = if reason.starts_with("policy:") {
            (
                6u8,
                "policy",
                vec![vec!["forgectl".into(), "status".into()]],
            )
        } else {
            match error.code {
                ErrorCode::NotFound => (
                    4,
                    "not_found",
                    vec![vec!["forgectl".into(), "run".into(), "list".into()]],
                ),
                ErrorCode::Conflict => (5, "conflict", conflict_next(&details)),
                ErrorCode::PreconditionFailed => (
                    5,
                    "precondition_failed",
                    vec![vec!["forgectl".into(), "task".into(), "show".into()]],
                ),
                _ => (1, code_name(error.code), vec![]),
            }
        };
        self.fail(code, name, &error.message, details, next)
    }

    fn client_error(&self, error: ClientError, mutation: bool) -> u8 {
        if mutation && matches!(error, ClientError::Timeout | ClientError::Disconnected) {
            return self.fail(
                3,
                "unreachable",
                &error.to_string(),
                json!({ "uncertain": true }),
                vec![],
            );
        }
        self.fail(3, "unreachable", &error.to_string(), json!({}), vec![])
    }

    fn print_ok(&self, result: Value) {
        if self.json {
            println!("{}", json!({ "ok": true, "result": result }));
        } else {
            println!(
                "{}",
                serde_json::to_string_pretty(&result).unwrap_or_default()
            );
        }
    }

    fn fail(
        &self,
        code: u8,
        name: &str,
        message: &str,
        details: Value,
        next: Vec<Vec<String>>,
    ) -> u8 {
        if self.json {
            println!(
                "{}",
                json!({
                    "ok": false,
                    "error": {
                        "code": name,
                        "message": message,
                        "details": details,
                        "next": next,
                    }
                })
            );
        } else {
            eprintln!("error: {message}");
            if let Some(argv) = next.first() {
                eprintln!("hint: {}", argv.join(" "));
            }
        }
        code
    }

    fn caller_json(&self) -> Value {
        json!({
            "session_id": env_session().map(|id| id.to_string()),
            "run_id": env_run().map(|id| id.to_string()),
            "task_id": env_task().map(|id| id.to_string()),
            "attempt_id": env_attempt().map(|id| id.to_string()),
        })
    }

    fn resolve_project(&self, client: &Client, raw: Option<&str>) -> Result<domain::ProjectId, u8> {
        let store = client
            .load_store()
            .map_err(|error| self.client_error(error, false))?;
        if let Some(raw) = raw {
            if let Ok(id) = raw.parse() {
                if store.projects.iter().any(|project| project.id == id) {
                    return Ok(id);
                }
            }
            if let Some(project) = store
                .projects
                .iter()
                .find(|project| project.name == raw || project.root_path.to_string_lossy() == raw)
            {
                return Ok(project.id);
            }
            return Err(self.fail(
                4,
                "not_found",
                "project not found",
                json!({}),
                vec![vec!["forgectl".into(), "run".into(), "list".into()]],
            ));
        }
        match store.projects.as_slice() {
            [only] => Ok(only.id),
            [] => Err(self.fail(
                2,
                "usage",
                "no project; pass --project",
                json!({}),
                vec![vec![
                    "forgectl".into(),
                    "run".into(),
                    "start".into(),
                    "--help".into(),
                ]],
            )),
            _ => Err(usage_code(
                self,
                "more than one project; pass --project",
                &["forgectl", "run", "start", "--help"],
            )),
        }
    }

    fn resolve_run(&self, client: &Client, raw: Option<&str>) -> Result<RunId, u8> {
        let raw = raw
            .map(str::to_owned)
            .or_else(|| env_run().map(|id| id.to_string()));
        let response = self.call(
            client,
            Request::ListRuns {
                project_id: None,
                active_only: false,
            },
            false,
        )?;
        let views = match response {
            Response::Runs(views) => views,
            _ => vec![],
        };
        let ids: Vec<RunId> = views.iter().map(|view| view.run.id).collect();
        match raw {
            Some(raw) => resolve_prefix(ids.iter(), &raw).copied().map_err(|_| {
                self.fail(
                    4,
                    "not_found",
                    "run not found",
                    json!({}),
                    vec![vec!["forgectl".into(), "run".into(), "list".into()]],
                )
            }),
            None if ids.len() == 1 => Ok(ids[0]),
            None => Err(usage_code(
                self,
                "no run; pass the run id or set FORGE_RUN_ID",
                &["forgectl", "run", "list"],
            )),
        }
    }

    fn find_task(&self, client: &Client, raw: &str) -> Result<(RunId, TaskId), u8> {
        let response = self.call(
            client,
            Request::ListRuns {
                project_id: None,
                active_only: false,
            },
            false,
        )?;
        let Response::Runs(views) = response else {
            return Err(self.fail(1, "internal", "run list", json!({}), vec![]));
        };
        let mut ids = Vec::new();
        for view in &views {
            for task in &view.tasks {
                ids.push((view.run.id, task.id));
            }
        }
        let task_ids: Vec<TaskId> = ids.iter().map(|(_, id)| *id).collect();
        let task_id = resolve_prefix(task_ids.iter(), raw).copied().map_err(|_| {
            self.fail(
                4,
                "not_found",
                "task not found",
                json!({}),
                vec![vec!["forgectl".into(), "task".into(), "list".into()]],
            )
        })?;
        let run_id = ids
            .into_iter()
            .find(|(_, id)| *id == task_id)
            .map(|(run, _)| run)
            .ok_or_else(|| self.fail(4, "not_found", "task not found", json!({}), vec![]))?;
        Ok((run_id, task_id))
    }

    fn resolve_tasks(
        &self,
        client: &Client,
        _run_id: RunId,
        raws: &[String],
    ) -> Result<Vec<TaskId>, u8> {
        raws.iter()
            .map(|raw| self.find_task(client, raw).map(|(_, id)| id))
            .collect()
    }

    fn resolve_session(&self, client: &Client, raw: &str) -> Result<SessionId, u8> {
        let store = client
            .load_store()
            .map_err(|error| self.client_error(error, false))?;
        let ids: Vec<SessionId> = store.sessions.iter().map(|session| session.id).collect();
        resolve_prefix(ids.iter(), raw).copied().map_err(|_| {
            self.fail(
                4,
                "not_found",
                "session not found",
                json!({}),
                vec![vec!["forgectl".into(), "session".into(), "list".into()]],
            )
        })
    }
}

fn usage_exit(message: &str, argv: &[&str]) -> ! {
    eprintln!("error: {message}");
    eprintln!("hint: {}", argv.join(" "));
    std::process::exit(2);
}

fn usage_code(app: &App, message: &str, argv: &[&str]) -> u8 {
    app.fail(
        2,
        "usage",
        message,
        json!({}),
        vec![argv.iter().map(|part| (*part).to_owned()).collect()],
    )
}

fn parse_duration(raw: &str) -> Result<Duration, String> {
    let raw = raw.trim();
    let (num, unit) = raw.split_at(raw.find(|c: char| !c.is_ascii_digit()).unwrap_or(raw.len()));
    let value: u64 = num.parse().map_err(|_| format!("bad duration {raw}"))?;
    match unit {
        "" | "s" => Ok(Duration::from_secs(value)),
        "m" => Ok(Duration::from_secs(value.saturating_mul(60))),
        "h" => Ok(Duration::from_secs(value.saturating_mul(60 * 60))),
        "ms" => Ok(Duration::from_millis(value)),
        _ => Err(format!("bad duration {raw}")),
    }
}

fn optional_profile(raw: Option<&str>) -> Result<Option<AgentProfileId>, String> {
    raw.map(|id| {
        id.parse::<AgentProfileId>()
            .map_err(|_| format!("profile id must be a uuid: {id}"))
    })
    .transpose()
}

fn parse_controller(
    raw: Option<&str>,
    have_session: bool,
) -> Result<domain::orchestration::ControllerSpec, String> {
    match raw.unwrap_or(if have_session { "self" } else { "none" }) {
        "none" => Ok(domain::orchestration::ControllerSpec::None),
        "self" => Ok(domain::orchestration::ControllerSpec::SelfSession),
        other => {
            let rest = other.strip_prefix("agent:").unwrap_or(other);
            let mut parts = rest.splitn(2, ':');
            let provider = parts.next().unwrap_or(rest);
            let profile = optional_profile(parts.next())?;
            Ok(domain::orchestration::ControllerSpec::Agent {
                provider_id: AgentProviderId::new(provider),
                profile_id: profile,
            })
        }
    }
}

fn parse_integration(raw: &str) -> Result<domain::orchestration::IntegrationPlacement, u8> {
    match raw {
        "new" => Ok(domain::orchestration::IntegrationPlacement::New),
        "current" => Ok(domain::orchestration::IntegrationPlacement::Current),
        other => other
            .parse::<WorkspaceId>()
            .map(domain::orchestration::IntegrationPlacement::Workspace)
            .map_err(|_| 2),
    }
}

fn parse_placement(raw: Option<&str>) -> Result<domain::orchestration::AttemptPlacement, u8> {
    match raw.unwrap_or("worktree") {
        "worktree" => Ok(domain::orchestration::AttemptPlacement::Worktree),
        "integration" => Ok(domain::orchestration::AttemptPlacement::Integration),
        "same" => Ok(domain::orchestration::AttemptPlacement::Same),
        other => other
            .parse::<WorkspaceId>()
            .map(domain::orchestration::AttemptPlacement::Workspace)
            .map_err(|_| 2),
    }
}

fn flatten(response: &Response) -> Value {
    if matches!(response, Response::Ack) {
        return json!({ "acked": true });
    }
    let value = serde_json::to_value(response).unwrap_or(Value::Null);
    match value {
        Value::Object(map) if map.len() == 1 => map.into_values().next().unwrap_or(Value::Null),
        Value::String(text) if text == "Ack" => json!({ "acked": true }),
        other => other,
    }
}

fn task_slice(view: &Value, task_id: TaskId) -> Value {
    let id = task_id.to_string();
    let task = view
        .get("tasks")
        .and_then(|v| v.as_array())
        .and_then(|tasks| {
            tasks
                .iter()
                .find(|task| task.get("id").and_then(|v| v.as_str()) == Some(id.as_str()))
                .cloned()
        });
    let attempt = view
        .get("attempts")
        .and_then(|v| v.as_array())
        .and_then(|attempts| {
            attempts
                .iter()
                .filter(|attempt| {
                    attempt.get("task_id").and_then(|v| v.as_str()) == Some(id.as_str())
                })
                .max_by_key(|attempt| attempt.get("n").and_then(|v| v.as_u64()).unwrap_or(0))
                .cloned()
        });
    json!({
        "task": task,
        "attempt": attempt,
        "status": task.as_ref().and_then(|task| task.get("status")).cloned().unwrap_or(Value::Null),
        "report": attempt.as_ref().and_then(|attempt| attempt.get("report")).cloned().unwrap_or(Value::Null),
    })
}

fn wake_items(view: &Value, kinds: &[WaitKind], tasks: &[TaskId]) -> Vec<Value> {
    let mut out = Vec::new();
    let attention = view
        .get("attention")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    for item in attention {
        let kind = item.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        let task_id = item.get("task_id").and_then(|v| v.as_str()).unwrap_or("");
        if !tasks.is_empty() && !tasks.iter().any(|id| id.to_string() == task_id) {
            continue;
        }
        let hit = kinds.iter().any(|wanted| match wanted {
            WaitKind::Attention => true,
            WaitKind::Reported => kind == "reported",
            WaitKind::Question => kind == "question",
            WaitKind::Settled => false,
        });
        if hit {
            out.push(item);
        }
    }
    if kinds.contains(&WaitKind::Settled) {
        let status = view
            .pointer("/run/status")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if matches!(status, "completed" | "cancelled") {
            out.push(json!({ "kind": "settled", "status": status }));
        }
    }
    out
}

fn conflict_next(details: &Value) -> Vec<Vec<String>> {
    if details.get("current_version").is_some() {
        vec![vec!["forgectl".into(), "state".into(), "get".into()]]
    } else {
        vec![vec!["forgectl".into(), "task".into(), "show".into()]]
    }
}

fn code_name(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::InvalidRequest => "invalid_request",
        ErrorCode::NotFound => "not_found",
        ErrorCode::Conflict => "conflict",
        ErrorCode::PreconditionFailed => "precondition_failed",
        ErrorCode::GitError => "git_error",
        ErrorCode::SpawnError => "spawn_error",
        ErrorCode::IoError => "io_error",
        ErrorCode::ProviderNotInstalled => "provider_not_installed",
        ErrorCode::ProtocolViolation => "protocol_violation",
        ErrorCode::Internal => "internal",
        ErrorCode::Unknown => "unknown",
        _ => "unknown",
    }
}

fn denied(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::PermissionDenied
        || matches!(error.raw_os_error(), Some(1 | 13))
}

fn wait_pull_request(client: &Client, budget: Duration) -> Option<DaemonEvent> {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        match client.events().recv_timeout(Duration::from_millis(200)) {
            Ok(event @ DaemonEvent::PullRequestOpened { .. }) => return Some(event),
            Ok(_) => {}
            Err(_) => {}
        }
    }
    None
}

fn read_text(text: Option<String>, file: Option<PathBuf>) -> Result<String, String> {
    if let Some(text) = text {
        return Ok(text);
    }
    if let Some(path) = file {
        return read_capped_text(&path, 1024 * 1024).map_err(|code| format!("exit {code}"));
    }
    Err("pass the text or a file".into())
}

fn read_capped_text(path: &Path, cap: usize) -> Result<String, u8> {
    let capped = client::read_capped(path, cap).map_err(|_| 2u8)?;
    String::from_utf8(capped.bytes).map_err(|_| 2u8)
}

fn env_session() -> Option<SessionId> {
    std::env::var("FORGE_SESSION_ID")
        .ok()
        .and_then(|s| s.parse().ok())
}
fn env_run() -> Option<RunId> {
    std::env::var("FORGE_RUN_ID")
        .ok()
        .and_then(|s| s.parse().ok())
}
fn env_task() -> Option<TaskId> {
    std::env::var("FORGE_TASK_ID")
        .ok()
        .and_then(|s| s.parse().ok())
}
fn env_attempt() -> Option<AttemptId> {
    std::env::var("FORGE_ATTEMPT_ID")
        .ok()
        .and_then(|s| s.parse().ok())
}
