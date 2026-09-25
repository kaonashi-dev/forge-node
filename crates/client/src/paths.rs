//! Daemon socket resolution shared by the CLI, the daemon, and the GUI locator.
//!
//! `FORGE_SOCKET` wins. Otherwise the platform runtime directory is used when
//! the full path fits in `sun_path`, and `/tmp/forge-$UID/daemon.sock` otherwise.

use std::path::{Path, PathBuf};

pub const MAX_SOCKET_PATH_LEN: usize = 100;

#[derive(Clone, Debug)]
pub struct SocketQuery {
    pub forge_socket: Option<PathBuf>,
    pub tmpdir: Option<PathBuf>,
    pub xdg_runtime_dir: Option<PathBuf>,
    pub uid: u32,
}

impl SocketQuery {
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            forge_socket: std::env::var_os("FORGE_SOCKET").map(PathBuf::from),
            tmpdir: std::env::var_os("TMPDIR")
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from),
            xdg_runtime_dir: std::env::var_os("XDG_RUNTIME_DIR")
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from),
            uid: current_uid(),
        }
    }
}

/// The socket a client should open.
pub fn resolve_socket(query: &SocketQuery) -> Result<PathBuf, String> {
    if let Some(path) = &query.forge_socket {
        return Ok(path.clone());
    }
    #[cfg(target_os = "linux")]
    if let Some(dir) = &query.xdg_runtime_dir {
        let path = dir.join("forge/daemon.sock");
        if fits(&path) {
            return Ok(path);
        }
    }
    #[cfg(target_os = "macos")]
    if let Some(dir) = &query.tmpdir {
        let path = dir.join("forge/daemon.sock");
        if fits(&path) {
            return Ok(path);
        }
    }
    let path = PathBuf::from(format!("/tmp/forge-{}/daemon.sock", query.uid));
    if fits(&path) {
        Ok(path)
    } else {
        Err(format!(
            "daemon socket path is too long: {}",
            path.display()
        ))
    }
}

pub fn socket_path() -> Result<PathBuf, String> {
    resolve_socket(&SocketQuery::from_env())
}

fn fits(path: &Path) -> bool {
    path.as_os_str().len() < MAX_SOCKET_PATH_LEN
}

fn current_uid() -> u32 {
    nix::unistd::Uid::current().as_raw()
}
