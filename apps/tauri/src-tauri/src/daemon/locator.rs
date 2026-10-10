//! Finds the daemon socket and binary and connects, starting the adjacent
//! `forge-daemon` if needed. Socket resolution comes from `client::resolve_socket`,
//! the same rule the daemon uses; only the daemon binary lookup is local.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use client::{Client, ClientError};

use crate::open::spawn_detached;

pub const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const CONNECT_RETRIES: usize = 60;
pub const CONNECT_RETRY: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, Default)]
pub struct Locator {
    socket_override: Option<PathBuf>,
    daemon_override: Option<PathBuf>,
    current_exe: Option<PathBuf>,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    tmpdir: Option<PathBuf>,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    xdg_runtime_dir: Option<PathBuf>,
    uid: u32,
}

impl Locator {
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            socket_override: std::env::var_os("FORGE_SOCKET").map(PathBuf::from),
            daemon_override: std::env::var_os("FORGE_DAEMON_BIN").map(PathBuf::from),
            current_exe: std::env::current_exe().ok(),
            tmpdir: std::env::var_os("TMPDIR")
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from),
            xdg_runtime_dir: std::env::var_os("XDG_RUNTIME_DIR")
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from),
            uid: nix::unistd::Uid::current().as_raw(),
        }
    }

    pub fn socket_path(&self) -> Result<PathBuf, String> {
        client::resolve_socket(&client::SocketQuery {
            forge_socket: self.socket_override.clone(),
            tmpdir: self.tmpdir.clone(),
            xdg_runtime_dir: self.xdg_runtime_dir.clone(),
            uid: self.uid,
        })
    }

    pub fn daemon_executable(&self) -> Result<PathBuf, String> {
        if let Some(path) = &self.daemon_override {
            return Ok(path.clone());
        }
        if let Some(current) = &self.current_exe {
            let sibling = current.with_file_name("forge-daemon");
            if sibling.is_file() {
                return Ok(sibling);
            }
        }
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        for profile in ["debug", "release"] {
            let candidate = manifest
                .join("../../../target")
                .join(profile)
                .join("forge-daemon");
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
        Err("forge-daemon is missing; build the daemon or set FORGE_DAEMON_BIN".to_string())
    }
}

pub fn connect_or_spawn(locator: &Locator) -> Result<Client, String> {
    let socket = locator.socket_path()?;
    match Client::connect(&socket, CLIENT_VERSION) {
        Ok(client) => return Ok(client),
        Err(error) if !daemon_absent(&error) => return Err(describe(&error)),
        Err(_) => {}
    }

    let daemon = locator.daemon_executable()?;
    // Its own group, so a terminal's Ctrl-C or a group-wide SIGINT aimed at this
    // app does not take every session down with it.
    spawn_detached(
        Command::new(&daemon)
            .arg("run")
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
    )
    .map_err(|error| format!("failed to start {}: {error}", daemon.display()))?;

    let mut last_error = None;
    for _ in 0..CONNECT_RETRIES {
        match Client::connect(&socket, CLIENT_VERSION) {
            Ok(client) => return Ok(client),
            Err(error @ ClientError::VersionMismatch { .. }) => return Err(describe(&error)),
            Err(error) => last_error = Some(error.to_string()),
        }
        thread::sleep(CONNECT_RETRY);
    }
    Err(format!(
        "daemon did not become ready: {}",
        last_error.unwrap_or_else(|| "unknown connection error".to_string())
    ))
}

/// Nothing is listening: a missing socket file, or one left by a dead daemon.
/// Any other failure means something answered, and a second daemon cannot help.
fn daemon_absent(error: &ClientError) -> bool {
    matches!(
        error,
        ClientError::Io(io) if matches!(
            io.kind(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
        )
    )
}

fn describe(error: &ClientError) -> String {
    match error {
        ClientError::VersionMismatch { expected, got } => format!(
            "the running forge-daemon speaks protocol {got} and this app speaks {expected}; \
             stop that daemon, then relaunch"
        ),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_socket_wins() {
        let locator = Locator {
            socket_override: Some(PathBuf::from("/tmp/custom.sock")),
            uid: 501,
            ..Locator::default()
        };
        assert_eq!(
            locator.socket_path().unwrap(),
            PathBuf::from("/tmp/custom.sock")
        );
    }

    #[test]
    fn fallback_socket_uses_uid() {
        let locator = Locator {
            uid: 501,
            ..Locator::default()
        };
        let path = locator.socket_path().unwrap();
        assert_eq!(path, PathBuf::from("/tmp/forge-501/daemon.sock"));
        assert!(path.as_os_str().len() < client::MAX_SOCKET_PATH_LEN);
    }

    #[test]
    fn only_an_absent_daemon_is_worth_starting_another() {
        use std::io::{Error, ErrorKind};

        assert!(daemon_absent(&ClientError::Io(Error::from(
            ErrorKind::NotFound
        ))));
        assert!(daemon_absent(&ClientError::Io(Error::from(
            ErrorKind::ConnectionRefused
        ))));
        assert!(!daemon_absent(&ClientError::Io(Error::from(
            ErrorKind::PermissionDenied
        ))));
        assert!(!daemon_absent(&ClientError::VersionMismatch {
            expected: 27,
            got: 26
        }));
        assert!(!daemon_absent(&ClientError::Disconnected));
    }

    #[test]
    fn a_version_mismatch_names_both_protocols() {
        let message = describe(&ClientError::VersionMismatch {
            expected: 27,
            got: 26,
        });
        assert!(
            message.contains("26") && message.contains("27"),
            "{message}"
        );
    }

    #[test]
    #[ignore = "requires a running forge-daemon"]
    fn connect_and_assert_session_count() {
        let client = connect_or_spawn(&Locator::from_env()).expect("connect");
        let store = client.load_store().expect("snapshot");
        assert!(
            !store.sessions.is_empty() || !store.workspaces.is_empty(),
            "connected daemon should have a workspace or a session"
        );
    }
}
