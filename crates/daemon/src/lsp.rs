//! Language servers for go to definition.
//!
//! One server per (server id, project root), started by the first lookup that
//! needs it and never by opening a file, so an agent's PTY or a snapshot
//! cannot launch one. At most `max_servers` live at once, idle ones are
//! stopped, and a crashing one backs off and then gives up. The map has its
//! own lock and never the core one; spawning, probing and requests run with
//! neither held.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use domain::ResolvedEnvironment;
use fs_service::definition::{
    Answer, Candidate, Confidence, DefinitionProvider, DefinitionQuery, Target,
};
use fs_service::FsError;
use lsp_client::{CrashHistory, Limits, Server, ServerConfig, Status};

use crate::config::LspConfig;

/// Documents kept open in one server; the least recently asked about is closed.
const MAX_OPEN_DOCUMENTS: usize = 64;

const PROBE_TIMEOUT_MS: u64 = 3_000;

/// How long a lookup waits for a server it just started before answering
/// without it. Short: the person is waiting, and the next lookup will find
/// the server ready.
const FIRST_ANSWER_WAIT: Duration = Duration::from_millis(1_500);

const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// Longest declaring line a candidate carries.
const MAX_LINE_BYTES: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    server: String,
    root: PathBuf,
}

struct Running {
    server: Arc<Server>,
    documents: Mutex<Documents>,
    /// Crashes before this start, so a restart loop is still bounded.
    history: CrashHistory,
}

#[derive(Default)]
struct Documents {
    /// Path, version, fingerprint of the synced text; most recent last.
    open: VecDeque<(PathBuf, i32, u64)>,
}

enum Slot {
    Running(Arc<Running>),
    /// Starting off the lock; a second lookup answers without it meanwhile.
    Starting,
    /// It crashed; restart after `retry_at`, or never once `retry_at` is `None`.
    Crashed {
        history: CrashHistory,
        retry_at: Option<Instant>,
        reason: String,
    },
}

/// A probe's verdict, keyed by what was probed.
#[derive(Clone)]
struct Probed {
    fingerprint: (u64, Option<SystemTime>),
    result: Result<(), String>,
}

/// The servers, their probes and their state.
pub(crate) struct Manager {
    config: LspConfig,
    registry: languages::Registry,
    slots: Mutex<HashMap<Key, Slot>>,
    probes: Mutex<HashMap<PathBuf, Probed>>,
}

impl Manager {
    pub(crate) fn new(config: &LspConfig) -> Self {
        Self {
            registry: config.registry(),
            config: config.clone(),
            slots: Mutex::new(HashMap::new()),
            probes: Mutex::new(HashMap::new()),
        }
    }

    #[must_use]
    pub(crate) fn enabled(&self) -> bool {
        self.config.enabled
    }

    /// Stop servers idle past `idle_shutdown_secs`; called by the idle sweeper.
    pub(crate) fn sweep(&self, now: Instant) {
        if self.config.idle_shutdown_secs == 0 {
            return;
        }
        let idle = Duration::from_secs(self.config.idle_shutdown_secs);
        let stopping: Vec<Arc<Running>> = {
            let mut slots = self.lock_slots();
            let expired: Vec<Key> = slots
                .iter()
                .filter_map(|(key, slot)| match slot {
                    Slot::Running(running)
                        if now.saturating_duration_since(running.server.last_used()) >= idle =>
                    {
                        Some(key.clone())
                    }
                    _ => None,
                })
                .collect();
            expired
                .into_iter()
                .filter_map(|key| match slots.remove(&key) {
                    Some(Slot::Running(running)) => Some(running),
                    _ => None,
                })
                .collect()
        };
        for running in stopping {
            tracing::info!(
                server = running.server.id(),
                "stopping an idle language server"
            );
            running.server.shutdown(SHUTDOWN_GRACE);
        }
    }

    /// Stop every server; the daemon is exiting.
    pub(crate) fn shutdown_all(&self) {
        let all: Vec<Arc<Running>> = self
            .lock_slots()
            .drain()
            .filter_map(|(_, slot)| match slot {
                Slot::Running(running) => Some(running),
                _ => None,
            })
            .collect();
        for running in all {
            running.server.shutdown(SHUTDOWN_GRACE);
        }
    }

    fn lock_slots(&self) -> std::sync::MutexGuard<'_, HashMap<Key, Slot>> {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The running server for `key`, starting it when allowed.
    ///
    /// `Err` is the note the editor shows while the other providers answer.
    fn ensure(
        &self,
        spec: &languages::ServerSpec,
        key: &Key,
        env: &ResolvedEnvironment,
    ) -> Result<Arc<Running>, String> {
        let now = Instant::now();
        let history = {
            let mut slots = self.lock_slots();
            let history = match slots.remove(key) {
                None => CrashHistory::default(),
                Some(Slot::Starting) => {
                    slots.insert(key.clone(), Slot::Starting);
                    return Err(format!("{} is starting", spec.id));
                }
                Some(Slot::Running(running)) => {
                    let Status::Exited { reason } = running.server.status() else {
                        slots.insert(key.clone(), Slot::Running(Arc::clone(&running)));
                        return Ok(running);
                    };
                    let mut history = running.history.clone();
                    history.record(now);
                    let retry_at = history.next_delay(now).map(|delay| now + delay);
                    let note = match retry_at {
                        Some(_) => format!("{} stopped ({reason}); restarting soon", spec.id),
                        None => format!("{} keeps crashing: {reason}", spec.id),
                    };
                    slots.insert(
                        key.clone(),
                        Slot::Crashed {
                            history,
                            retry_at,
                            reason,
                        },
                    );
                    return Err(note);
                }
                Some(Slot::Crashed {
                    history,
                    retry_at,
                    reason,
                }) => match retry_at {
                    Some(at) if at <= now => history,
                    _ => {
                        let note = match retry_at {
                            Some(_) => format!("{} crashed; restarting soon", spec.id),
                            None => format!("{} keeps crashing: {reason}", spec.id),
                        };
                        slots.insert(
                            key.clone(),
                            Slot::Crashed {
                                history,
                                retry_at,
                                reason,
                            },
                        );
                        return Err(note);
                    }
                },
            };
            slots.insert(key.clone(), Slot::Starting);
            history
        };
        let started = self.start(spec, key, env, history);
        let evicted = {
            let mut slots = self.lock_slots();
            match &started {
                Ok(running) => {
                    slots.insert(key.clone(), Slot::Running(Arc::clone(running)));
                }
                Err(_) => {
                    slots.remove(key);
                }
            }
            self.evict_over_budget(&mut slots, key)
        };
        for running in evicted {
            running.server.shutdown(SHUTDOWN_GRACE);
        }
        started
    }

    /// The least recently used servers past `max_servers`, never `keep`.
    fn evict_over_budget(&self, slots: &mut HashMap<Key, Slot>, keep: &Key) -> Vec<Arc<Running>> {
        let mut running: Vec<(Instant, Key)> = slots
            .iter()
            .filter_map(|(key, slot)| match slot {
                Slot::Running(running) if key != keep => {
                    Some((running.server.last_used(), key.clone()))
                }
                _ => None,
            })
            .collect();
        let budget = self.config.max_servers.max(1).saturating_sub(1);
        if running.len() <= budget {
            return Vec::new();
        }
        running.sort_by_key(|(used, _)| *used);
        let excess = running.len() - budget;
        running
            .into_iter()
            .take(excess)
            .filter_map(|(_, key)| match slots.remove(&key) {
                Some(Slot::Running(running)) => Some(running),
                _ => None,
            })
            .collect()
    }

    fn start(
        &self,
        spec: &languages::ServerSpec,
        key: &Key,
        env: &ResolvedEnvironment,
        history: CrashHistory,
    ) -> Result<Arc<Running>, String> {
        let command = resolve(&spec.command, env)
            .ok_or_else(|| format!("{} is not installed ({})", spec.id, spec.command))?;
        self.probe(spec, &command, env)?;
        let config = ServerConfig {
            id: spec.id.clone(),
            command,
            args: spec.args.clone(),
            root: key.root.clone(),
            env: server_env(env),
            initialization_options: spec.initialization_options.clone(),
            settings: spec.settings.clone(),
            limits: Limits {
                initialize_timeout: Duration::from_secs(self.config.initialize_timeout_secs.max(1)),
                ..Limits::default()
            },
        };
        tracing::info!(server = %spec.id, root = %key.root.display(), "starting a language server");
        let server = Server::start(config).map_err(|error| format!("{}: {error}", spec.id))?;
        Ok(Arc::new(Running {
            server: Arc::new(server),
            documents: Mutex::new(Documents::default()),
            history,
        }))
    }

    /// Run the server's version probe once per binary, again only when the
    /// binary changes on disk. No server launches unprobed.
    fn probe(
        &self,
        spec: &languages::ServerSpec,
        command: &Path,
        env: &ResolvedEnvironment,
    ) -> Result<(), String> {
        let program = match &spec.probe.command {
            Some(other) => resolve(other, env)
                .ok_or_else(|| format!("{} is not installed ({other})", spec.id))?,
            None => command.to_path_buf(),
        };
        let fingerprint = std::fs::metadata(&program)
            .map(|meta| (meta.len(), meta.modified().ok()))
            .map_err(|error| format!("{}: {error}", program.display()))?;
        if let Some(known) = self
            .probes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&program)
            .filter(|known| known.fingerprint == fingerprint)
        {
            return known.result.clone();
        }
        let result = match agents::detection::run_to_completion(
            &program,
            &spec.probe.args,
            PROBE_TIMEOUT_MS,
            env,
        ) {
            Some(stdout) if String::from_utf8_lossy(&stdout).contains(&spec.probe.expect) => Ok(()),
            Some(_) => Err(format!(
                "{} did not identify itself as {}",
                program.display(),
                spec.id
            )),
            None => Err(format!("{} failed its version probe", spec.id)),
        };
        self.probes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                program,
                Probed {
                    fingerprint,
                    result: result.clone(),
                },
            );
        result
    }
}

/// A bare name is looked up on the login shell's `PATH`, like an agent CLI.
fn resolve(command: &str, env: &ResolvedEnvironment) -> Option<PathBuf> {
    let path = Path::new(command);
    if path.is_absolute() {
        return path.is_file().then(|| path.to_path_buf());
    }
    if path.components().count() == 1 {
        return agents::detection::find_executable(command, &env.path_entries);
    }
    None
}

/// The login environment, without the `FORGE_*` a session would get: a
/// language server is not a session and must not report into one.
fn server_env(env: &ResolvedEnvironment) -> Vec<(String, String)> {
    let mut vars: Vec<(String, String)> = env
        .vars
        .iter()
        .filter(|(key, _)| !key.starts_with("FORGE_"))
        .cloned()
        .collect();
    if !vars.iter().any(|(key, _)| key == "PATH") {
        if let Ok(joined) = std::env::join_paths(&env.path_entries) {
            vars.push(("PATH".to_owned(), joined.to_string_lossy().into_owned()));
        }
    }
    vars
}

/// Go to definition through a language server, as one link of the chain.
pub(crate) struct LspProvider<'a> {
    pub manager: &'a Manager,
    pub env: &'a ResolvedEnvironment,
    pub definition_timeout: Duration,
}

impl DefinitionProvider for LspProvider<'_> {
    fn name(&self) -> &'static str {
        "lsp"
    }

    fn find(&self, query: &DefinitionQuery<'_>) -> Result<Answer, FsError> {
        let Some(position) = query.position else {
            return Ok(Answer::default());
        };
        let Some(spec) = self.manager.registry.servers_for_path(query.source).next() else {
            return Ok(Answer::default());
        };
        let Some(language) = self.manager.registry.language_for_path(query.source) else {
            return Ok(Answer::default());
        };
        let file = query.root.join(query.source);
        let key = Key {
            server: spec.id.clone(),
            root: languages::find_root(spec, query.root, &file, Path::exists),
        };
        let running = match self.manager.ensure(spec, &key, self.env) {
            Ok(running) => running,
            Err(note) => return Ok(note_only(note)),
        };
        let server = &running.server;
        let wait = (Instant::now() + FIRST_ANSWER_WAIT).min(query.deadline);
        match server.wait_ready(wait) {
            Status::Ready => {}
            Status::Starting => return Ok(note_only(format!("{} is starting", spec.id))),
            Status::Indexing { title, percentage } => {
                let progress = percentage.map_or(String::new(), |p| format!(" {p}%"));
                return Ok(note_only(format!(
                    "{} is indexing ({title}{progress})",
                    spec.id
                )));
            }
            Status::Exited { reason } => {
                return Ok(note_only(format!("{} stopped: {reason}", spec.id)));
            }
        }
        let text = match query.buffer {
            Some(text) => text.to_owned(),
            None => fs_service::read_file(query.root, query.source)?.text,
        };
        sync(&running, &file, language.lsp_id(), &text);
        let deadline = (Instant::now() + self.definition_timeout).min(query.deadline);
        let found = server.definition(
            &file,
            position.line.saturating_sub(1),
            position.utf16_column.saturating_sub(1),
            deadline,
            query.cancel,
        );
        let locations = match found {
            Ok(locations) => locations,
            Err(lsp_client::LspError::Timeout) => {
                return Ok(note_only(format!("{} did not answer in time", spec.id)));
            }
            // ContentModified: the server is still loading the project, which
            // rust-analyzer reports this way before its first progress token.
            Err(lsp_client::LspError::ResponseError { code: -32801, .. }) => {
                return Ok(note_only(format!("{} is still loading", spec.id)));
            }
            Err(error) => {
                tracing::debug!(server = %spec.id, %error, "definition request failed");
                return Ok(Answer::default());
            }
        };
        let candidates: Vec<Candidate> = locations
            .into_iter()
            .take(query.limit)
            .filter_map(|location| candidate(query.root, location))
            .collect();
        Ok(Answer {
            authoritative: !candidates.is_empty(),
            candidates,
            truncated: false,
            note: None,
        })
    }
}

fn note_only(note: String) -> Answer {
    Answer {
        note: Some(note),
        ..Answer::default()
    }
}

/// Open the document in the server, or send its new text when it changed.
fn sync(running: &Running, file: &Path, language_id: &str, text: &str) {
    let fingerprint = {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        hasher.finish()
    };
    let mut documents = running
        .documents
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let known = documents.open.iter().position(|(path, ..)| path == file);
    match known {
        Some(index) => {
            let Some((path, version, seen)) = documents.open.remove(index) else {
                return;
            };
            let version = if seen == fingerprint {
                version
            } else {
                running.server.did_change_full(&path, version + 1, text);
                version + 1
            };
            documents.open.push_back((path, version, fingerprint));
        }
        None => {
            if documents.open.len() >= MAX_OPEN_DOCUMENTS {
                if let Some((oldest, ..)) = documents.open.pop_front() {
                    running.server.did_close(&oldest);
                }
            }
            running.server.did_open(file, language_id, 1, text);
            documents
                .open
                .push_back((file.to_path_buf(), 1, fingerprint));
        }
    }
}

/// A location as a candidate: inside the checkout by its relative path,
/// outside it by its canonical one. A location that no longer exists is
/// dropped rather than offered.
fn candidate(root: &Path, location: lsp_client::Location) -> Option<Candidate> {
    let path = location.path.canonicalize().ok()?;
    let line = location.line.saturating_add(1);
    let text = declaring_line(&path, location.line as usize).unwrap_or_default();
    let target = match path.strip_prefix(root) {
        Ok(relative) => Target::Workspace(relative.to_str()?.to_owned()),
        Err(_) => Target::External(path),
    };
    Some(Candidate {
        target,
        line,
        column: location.character.saturating_add(1),
        text,
        confidence: Confidence::Semantic,
    })
}

/// Line `index` (0-based) of a text file within the document budget.
fn declaring_line(path: &Path, index: usize) -> Option<String> {
    use std::io::Read;
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > fs_service::MAX_FILE_BYTES as u64 {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(fs_service::MAX_FILE_BYTES as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let line = text.lines().nth(index)?.trim();
    let mut end = line.len().min(MAX_LINE_BYTES);
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    Some(line[..end].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment(vars: Vec<(String, String)>, path_entries: Vec<PathBuf>) -> ResolvedEnvironment {
        ResolvedEnvironment {
            shell: PathBuf::from("/bin/sh"),
            vars,
            path_entries,
            resolved_at: domain::Timestamp::now(),
            source: domain::EnvSource::LoginShell,
        }
    }

    #[test]
    fn server_environments_never_carry_forge_session_variables() {
        let env = environment(
            vec![
                ("HOME".into(), "/h".into()),
                ("FORGE_SESSION_ID".into(), "x".into()),
            ],
            vec![PathBuf::from("/usr/bin")],
        );
        let vars = server_env(&env);
        assert!(vars.iter().any(|(k, _)| k == "HOME"));
        assert!(vars.iter().all(|(k, _)| !k.starts_with("FORGE_")));
        assert!(vars.iter().any(|(k, v)| k == "PATH" && v == "/usr/bin"));
    }

    #[test]
    fn candidates_inside_the_checkout_are_relative_and_outside_are_absolute() {
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();
        std::fs::write(root_path.join("lib.rs"), "\nfn thing() {}\n").unwrap();
        let other = tempfile::tempdir().unwrap();
        let outside = other.path().canonicalize().unwrap().join("dep.rs");
        std::fs::write(&outside, "pub fn dep() {}\n").unwrap();

        let inside = candidate(
            &root_path,
            lsp_client::Location {
                path: root_path.join("lib.rs"),
                line: 1,
                character: 3,
            },
        )
        .unwrap();
        assert_eq!(inside.target, Target::Workspace("lib.rs".into()));
        assert_eq!((inside.line, inside.column), (2, 4));
        assert_eq!(inside.text, "fn thing() {}");

        let away = candidate(
            &root_path,
            lsp_client::Location {
                path: outside.clone(),
                line: 0,
                character: 7,
            },
        )
        .unwrap();
        assert_eq!(away.target, Target::External(outside));
        assert!(candidate(
            &root_path,
            lsp_client::Location {
                path: root_path.join("gone.rs"),
                line: 0,
                character: 0,
            },
        )
        .is_none());
    }

    const FAKE_SERVER: &str = r#"#!/usr/bin/python3
import json, sys
if sys.argv[1:] == ["--version"]:
    print("fake-lsp 1.0")
    sys.exit(0)
inp, out = sys.stdin.buffer, sys.stdout.buffer
def send(message):
    body = json.dumps(message).encode()
    out.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    out.flush()
opened = {}
while True:
    length = 0
    while True:
        line = inp.readline()
        if not line:
            sys.exit(0)
        if line in (b"\r\n", b"\n"):
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":")[1])
    message = json.loads(inp.read(length))
    method = message.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": message["id"], "result": {"capabilities": {"definitionProvider": True}}})
    elif method == "textDocument/didOpen":
        opened[message["params"]["textDocument"]["uri"]] = message["params"]["textDocument"]["text"]
    elif method == "textDocument/didChange":
        opened[message["params"]["textDocument"]["uri"]] = message["params"]["contentChanges"][-1]["text"]
    elif method == "textDocument/definition":
        uri = message["params"]["textDocument"]["uri"]
        target = uri.rsplit("/", 1)[0] + "/lib.rs"
        line = 1 if "unsaved" in opened.get(uri, "") else 0
        send({"jsonrpc": "2.0", "id": message["id"], "result": [
            {"uri": target, "range": {"start": {"line": line, "character": 7}, "end": {"line": line, "character": 13}}}]})
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": message["id"], "result": None})
    elif method == "exit":
        sys.exit(0)
"#;

    /// The whole path a lookup takes through a server: probe, start, open the
    /// buffer the editor sent, ask, and turn the answer into a candidate.
    #[test]
    fn a_probed_server_answers_for_the_buffer_and_is_stopped_on_shutdown() {
        if !Path::new("/usr/bin/python3").exists() {
            return;
        }
        let scratch = tempfile::tempdir().unwrap();
        let server = scratch.path().join("fake-lsp");
        std::fs::write(&server, FAKE_SERVER).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&server, std::fs::Permissions::from_mode(0o755)).unwrap();
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();
        std::fs::write(
            root_path.join("lib.rs"),
            "pub fn answer() {}\npub fn second() {}\n",
        )
        .unwrap();
        std::fs::write(root_path.join("a.rs"), "answer();\n").unwrap();

        let overrides: languages::Overrides = toml::from_str(&format!(
            r#"
            [servers.fake]
            command = "{}"
            probe = {{ args = ["--version"], expect = "fake-lsp" }}
            [languages.rust]
            servers = ["fake"]
            "#,
            server.display()
        ))
        .unwrap();
        let manager = Manager::new(&LspConfig {
            enabled: true,
            languages: overrides.languages,
            servers: overrides.servers,
            ..LspConfig::default()
        });
        let env = environment(
            vec![("PATH".into(), "/usr/bin:/bin".into())],
            vec![PathBuf::from("/usr/bin")],
        );
        let provider = LspProvider {
            manager: &manager,
            env: &env,
            definition_timeout: Duration::from_secs(5),
        };
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let ask = |buffer: Option<&str>| {
            provider.find(&DefinitionQuery {
                root: &root_path,
                source: "a.rs",
                symbol: "answer",
                position: Some(fs_service::definition::Position {
                    line: 1,
                    utf16_column: 1,
                }),
                buffer,
                limit: 8,
                deadline: Instant::now() + Duration::from_secs(10),
                cancel: &cancel,
            })
        };
        let answer = ask(None).unwrap();
        assert!(answer.authoritative, "{answer:?}");
        assert_eq!(answer.candidates.len(), 1);
        let found = &answer.candidates[0];
        assert_eq!(found.target, Target::Workspace("lib.rs".into()));
        assert_eq!((found.line, found.column), (1, 8));
        assert_eq!(found.text, "pub fn answer() {}");
        assert_eq!(found.confidence, Confidence::Semantic);

        let unsaved = ask(Some("unsaved();\n")).unwrap();
        assert_eq!(
            unsaved.candidates[0].line, 2,
            "the server saw the editor's buffer"
        );

        let pid = {
            let slots = manager.lock_slots();
            match slots.values().next() {
                Some(Slot::Running(running)) => running.server.process_id(),
                _ => panic!("the server should be running"),
            }
        };
        manager.shutdown_all();
        let gone = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(-i32::try_from(pid).unwrap()),
            None,
        );
        assert!(
            gone.is_err(),
            "the server's group must be gone after shutdown"
        );
    }

    #[test]
    fn a_disabled_manager_or_an_unknown_language_answers_nothing() {
        let manager = Manager::new(&LspConfig::default());
        assert!(!manager.enabled());
        let env = environment(Vec::new(), Vec::new());
        let provider = LspProvider {
            manager: &manager,
            env: &env,
            definition_timeout: Duration::from_millis(100),
        };
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let query = DefinitionQuery {
            root: Path::new("/nowhere"),
            source: "notes.unknown",
            symbol: "x",
            position: Some(fs_service::definition::Position {
                line: 1,
                utf16_column: 1,
            }),
            buffer: None,
            limit: 4,
            deadline: Instant::now() + Duration::from_secs(1),
            cancel: &cancel,
        };
        assert_eq!(provider.find(&query).unwrap(), Answer::default());
    }
}
