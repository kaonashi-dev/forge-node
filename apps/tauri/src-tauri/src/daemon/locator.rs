//! Socket and daemon binary resolution
//! (`socket_path`, `daemon_executable`, `connect_or_spawn` constants).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use client::Client;

pub const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const CONNECT_RETRIES: usize = 60;
pub const CONNECT_RETRY: Duration = Duration::from_millis(50);
const MAX_SOCKET_PATH: usize = 100;

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
        if let Some(path) = &self.socket_override {
            return Ok(path.clone());
        }

        #[cfg(target_os = "linux")]
        if let Some(dir) = &self.xdg_runtime_dir {
            let path = dir.join("forge/daemon.sock");
            if fits(&path) {
                return Ok(path);
            }
        }
        #[cfg(target_os = "macos")]
        if let Some(dir) = &self.tmpdir {
            let path = dir.join("forge/daemon.sock");
            if fits(&path) {
                return Ok(path);
            }
        }

        let path = PathBuf::from(format!("/tmp/forge-{}/daemon.sock", self.uid));
        if fits(&path) {
            Ok(path)
        } else {
            Err(format!(
                "daemon socket path is too long: {}",
                path.display()
            ))
        }
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
    if let Ok(client) = Client::connect(&socket, CLIENT_VERSION) {
        return Ok(client);
    }

    let daemon = locator.daemon_executable()?;
    Command::new(&daemon)
        .arg("run")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("failed to start {}: {error}", daemon.display()))?;

    let mut last_error = None;
    for _ in 0..CONNECT_RETRIES {
        match Client::connect(&socket, CLIENT_VERSION) {
            Ok(client) => return Ok(client),
            Err(error) => last_error = Some(error.to_string()),
        }
        thread::sleep(CONNECT_RETRY);
    }
    Err(format!(
        "daemon did not become ready: {}",
        last_error.unwrap_or_else(|| "unknown connection error".to_string())
    ))
}

fn fits(path: &Path) -> bool {
    path.as_os_str().len() < MAX_SOCKET_PATH
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
        assert!(path.as_os_str().len() < MAX_SOCKET_PATH);
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
