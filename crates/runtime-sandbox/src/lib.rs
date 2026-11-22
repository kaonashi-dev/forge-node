//! Sandbox checks: a repository's `.forge/review/checks.toml` targets run as written,
//! under macOS `sandbox-exec`, with no network, writes only to the run, the
//! cache and the temp directory, no view of Forge's own socket or home, and a
//! cleared environment. Each target is its own process group, killed whole when
//! its leader exits or times out, and only the tail of its output is kept.

use std::collections::VecDeque;
use std::io::Read;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

pub const CONFIG_PATH: &str = ".forge/review/checks.toml";
/// `CONFIG_PATH`'s earlier name: never read, only pointed out by [`missing`].
const RENAMED_FROM: &str = ".forge/review/sandbox.toml";
pub const MAX_CONFIG_BYTES: u64 = 16 * 1024;
pub const UNSUPPORTED: &str = "Sandbox checks are unsupported on this platform";
const DEFAULT_TIMEOUT_SECS: u64 = 600;
const MAX_TIMEOUT_SECS: u64 = 3600;
const MAX_TARGETS: usize = 32;
/// Bytes kept from the end of each of stdout and stderr.
pub const TAIL_BYTES: usize = 8 * 1024;
pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";
/// How long a killed target's pipes and exit are waited for before giving up on it.
const REAP_GRACE: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub name: String,
    pub command: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxConfig {
    pub timeout: Duration,
    /// Fixed values, never inherited.
    pub env: Vec<(String, String)>,
    pub targets: Vec<Target>,
}

/// Why `checkout` has nothing to run, naming the rename when it still has the earlier file.
#[must_use]
pub fn missing(checkout: &Path) -> String {
    if checkout.join(RENAMED_FROM).exists() {
        format!("no {CONFIG_PATH}; rename {RENAMED_FROM} to it")
    } else {
        format!("no {CONFIG_PATH}")
    }
}

/// # Errors
/// Invalid TOML, a target without a name or a command, a duplicate name, or none at all.
pub fn parse(text: &str) -> Result<SandboxConfig, String> {
    let table: toml::Table = text.parse().map_err(|error: toml::de::Error| {
        format!("checks.toml is not valid TOML: {}", error.message())
    })?;
    let timeout = match table.get("timeout_secs") {
        None => DEFAULT_TIMEOUT_SECS,
        Some(toml::Value::Integer(n)) if *n > 0 => u64::try_from(*n)
            .unwrap_or(MAX_TIMEOUT_SECS)
            .min(MAX_TIMEOUT_SECS),
        Some(_) => return Err("checks.toml: `timeout_secs` must be a positive integer".into()),
    };
    let mut env = Vec::new();
    match table.get("env") {
        None => {}
        Some(toml::Value::Table(values)) => {
            for (name, value) in values {
                let value = value
                    .as_str()
                    .ok_or_else(|| format!("checks.toml: `[env] {name}` must be a string"))?;
                env.push((name.clone(), value.to_owned()));
            }
        }
        Some(_) => return Err("checks.toml: `[env]` must be a table".into()),
    }
    let mut targets: Vec<Target> = Vec::new();
    let listed = table
        .get("targets")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "checks.toml: no `[[targets]]`".to_owned())?;
    for item in listed.iter().take(MAX_TARGETS) {
        let field = |key: &str| {
            item.get(key)
                .and_then(toml::Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
        };
        let (Some(name), Some(command)) = (field("name"), field("command")) else {
            return Err("checks.toml: every target needs a `name` and a `command`".into());
        };
        if targets.iter().any(|t| t.name == name) {
            return Err(format!("checks.toml: target `{name}` is listed twice"));
        }
        targets.push(Target {
            name: name.to_owned(),
            command: command.to_owned(),
        });
    }
    if targets.is_empty() {
        return Err("checks.toml: no `[[targets]]`".into());
    }
    Ok(SandboxConfig {
        timeout: Duration::from_secs(timeout),
        env,
        targets,
    })
}

/// What one run may touch. Every path should already be canonical: the
/// sandbox matches real paths, and `/tmp` is `/private/tmp` on macOS.
#[derive(Clone, Debug)]
pub struct Policy {
    pub run_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub tmp_dir: PathBuf,
    /// Writable too, but unlike the run and cache directories never exempt from `denied_reads`.
    pub extra_writes: Vec<PathBuf>,
    /// Forge's socket directory and home.
    pub denied_reads: Vec<PathBuf>,
    pub network: bool,
    /// Forge's socket, unreachable even with the network on.
    pub socket: Option<PathBuf>,
}

fn quoted(path: &Path) -> String {
    let text = path.to_string_lossy();
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The built-in SBPL profile for `policy`.
#[must_use]
pub fn profile(policy: &Policy) -> String {
    let mut out = String::from("(version 1)\n(allow default)\n");
    if !policy.network {
        out.push_str("(deny network*)\n");
    }
    if let Some(socket) = &policy.socket {
        out.push_str(&format!(
            "(deny network-outbound (remote unix-socket (path-literal {})))\n",
            quoted(socket)
        ));
    }
    let mut writable = String::new();
    for path in [&policy.run_dir, &policy.cache_dir, &policy.tmp_dir]
        .into_iter()
        .chain(&policy.extra_writes)
    {
        writable.push_str(&format!("(subpath {}) ", quoted(path)));
    }
    out.push_str(&format!(
        "(deny file-write* (require-not (require-any {writable}(subpath \"/dev\"))))\n"
    ));
    // Data, not metadata: a shell resolves its directory through every
    // ancestor. The run and cache directories stay reachable even inside a
    // denied one, as a pull request's head or a managed worktree under
    // Forge's home is.
    for path in &policy.denied_reads {
        out.push_str(&format!(
            "(deny file-read-data file-write* (require-all (subpath {}) (require-not (require-any (subpath {}) (subpath {})))))\n",
            quoted(path),
            quoted(&policy.run_dir),
            quoted(&policy.cache_dir),
        ));
    }
    out
}

/// The target's environment: only `whitelist`'s names from `resolved`, then
/// the file's fixed values, then the cache directory. Nothing named `FORGE_*`
/// from either, so `FORGE_SANDBOX_CACHE` is the only one.
#[must_use]
pub fn environment(
    whitelist: &[String],
    resolved: &dyn Fn(&str) -> Option<String>,
    fixed: &[(String, String)],
    cache_dir: &Path,
) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = whitelist
        .iter()
        .filter(|name| !name.starts_with("FORGE_"))
        .filter_map(|name| Some((name.clone(), resolved(name)?)))
        .collect();
    for (name, value) in fixed.iter().filter(|(name, _)| !name.starts_with("FORGE_")) {
        env.retain(|(other, _)| other != name);
        env.push((name.clone(), value.clone()));
    }
    env.push((
        "FORGE_SANDBOX_CACHE".into(),
        cache_dir.to_string_lossy().into_owned(),
    ));
    env
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetResult {
    pub name: String,
    /// `None` when it was killed: by its timeout or a signal.
    pub exit_code: Option<i32>,
    pub duration: Duration,
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
}

impl TargetResult {
    #[must_use]
    pub fn passed(&self) -> bool {
        self.exit_code == Some(0)
    }
}

/// The last `cap` bytes a pipe carried.
#[derive(Default)]
struct Tail {
    bytes: VecDeque<u8>,
}

fn drain(pipe: Option<impl Read>, tail: Arc<Mutex<Tail>>) {
    let Some(mut pipe) = pipe else { return };
    let mut chunk = [0u8; 4096];
    while let Ok(read) = pipe.read(&mut chunk) {
        if read == 0 {
            break;
        }
        let mut tail = tail
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        tail.bytes.extend(&chunk[..read]);
        let over = tail.bytes.len().saturating_sub(TAIL_BYTES);
        tail.bytes.drain(..over);
    }
}

fn text(tail: &Arc<Mutex<Tail>>) -> String {
    let tail = tail
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let bytes: Vec<u8> = tail.bytes.iter().copied().collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Blocks until `pid` has exited, without reaping it.
fn await_exit(pid: u32) {
    let id: libc::id_t = pid;
    let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
    loop {
        // SAFETY: `info` is valid, writable storage for one `siginfo_t` for the whole call.
        let rc = unsafe {
            libc::waitid(
                libc::P_PID,
                id,
                info.as_mut_ptr(),
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if rc == 0 || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return;
        }
    }
}

/// `leader` is still unreaped, because only [`reap`] reaps and it takes the
/// `Child`: its pid and process group cannot have been recycled.
fn kill_group(leader: &Child) {
    #[allow(
        clippy::cast_possible_wrap,
        reason = "a pid always fits in i32 on unix"
    )]
    let raw = leader.id() as i32;
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(-raw),
        nix::sys::signal::Signal::SIGKILL,
    );
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(raw),
        nix::sys::signal::Signal::SIGKILL,
    );
}

/// The leader's status if it has exited; otherwise a thread waits for it.
fn reap(mut leader: Child) -> Option<ExitStatus> {
    match leader.try_wait() {
        Ok(Some(status)) => Some(status),
        Ok(None) => {
            std::thread::spawn(move || {
                let _ = leader.wait();
            });
            None
        }
        Err(_) => None,
    }
}

/// Run one target in `policy.run_dir`.
///
/// # Errors
/// Not macOS, or the sandbox could not be started.
pub fn run_target(
    target: &Target,
    timeout: Duration,
    policy: &Policy,
    env: &[(String, String)],
) -> Result<TargetResult, String> {
    if !cfg!(target_os = "macos") {
        return Err(UNSUPPORTED.into());
    }
    let started = Instant::now();
    let mut child = Command::new(SANDBOX_EXEC)
        .arg("-p")
        .arg(profile(policy))
        .arg("/bin/sh")
        .arg("-c")
        .arg(&target.command)
        .current_dir(&policy.run_dir)
        .env_clear()
        .envs(env.iter().map(|(name, value)| (name, value)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|error| format!("could not start {SANDBOX_EXEC}: {error}"))?;
    let pid = child.id();
    let (out, err) = (
        Arc::new(Mutex::new(Tail::default())),
        Arc::new(Mutex::new(Tail::default())),
    );
    let (drained, finished) = mpsc::channel();
    {
        let (pipe, tail, drained) = (child.stdout.take(), Arc::clone(&out), drained.clone());
        std::thread::spawn(move || {
            drain(pipe, tail);
            let _ = drained.send(());
        });
    }
    {
        let (pipe, tail) = (child.stderr.take(), Arc::clone(&err));
        std::thread::spawn(move || {
            drain(pipe, tail);
            let _ = drained.send(());
        });
    }
    let (exited_tx, exited) = mpsc::channel();
    std::thread::spawn(move || {
        await_exit(pid);
        let _ = exited_tx.send(());
    });
    let timed_out = exited.recv_timeout(timeout).is_err();
    // After a normal exit too: a backgrounded descendant would keep running.
    kill_group(&child);
    if timed_out {
        let _ = exited.recv_timeout(REAP_GRACE);
    }
    let status = reap(child);
    // A descendant that left the group could hold a pipe open; its output is
    // read for as long as the grace allows and the rest is left behind.
    let deadline = Instant::now() + REAP_GRACE;
    for _ in 0..2 {
        let left = deadline.saturating_duration_since(Instant::now());
        if finished.recv_timeout(left).is_err() {
            break;
        }
    }
    Ok(TargetResult {
        name: target.name.clone(),
        exit_code: if timed_out {
            None
        } else {
            status.and_then(|s| s.code())
        },
        duration: started.elapsed(),
        timed_out,
        stdout: text(&out),
        stderr: text(&err),
    })
}

#[cfg(test)]
mod tests;
