//! A blocking Language Server Protocol client: one [`Server`] per launched
//! process, driven from ordinary threads with bounded queues, clamped frames
//! and process-group teardown, so a daemon can ask for definitions without
//! an async runtime.

mod framing;
mod location;
mod restart;
mod server;
mod state;
mod uri;

use std::path::PathBuf;
use std::time::Duration;

pub use location::MAX_LOCATIONS;
pub use restart::{CrashHistory, CRASH_WINDOW, GIVE_UP_AFTER};
pub use server::Server;
pub use uri::{path_to_uri, uri_to_path};

/// How to launch one language server.
#[derive(Clone, Debug)]
pub struct ServerConfig {
    /// Names the server in logs, e.g. `rust-analyzer`.
    pub id: String,
    /// Resolved executable; it is not looked up on `PATH`.
    pub command: PathBuf,
    pub args: Vec<String>,
    /// Canonical workspace root: the process's working directory, `rootUri`
    /// and the single workspace folder.
    pub root: PathBuf,
    /// The complete environment; nothing is inherited from the daemon.
    pub env: Vec<(String, String)>,
    pub initialization_options: serde_json::Value,
    /// Answers `workspace/configuration`: each requested section is looked up
    /// as a literal key, then as a dotted path, and is `null` when missing.
    pub settings: serde_json::Value,
    pub limits: Limits,
}

#[derive(Clone, Debug)]
pub struct Limits {
    /// Largest message body held; a longer one is read into a sink.
    pub max_message_bytes: usize,
    /// From spawn to the `initialize` answer.
    pub initialize_timeout: Duration,
    /// Requests in flight at once; past it a request fails with [`LspError::Busy`].
    pub max_pending: usize,
    /// Tail of the server's stderr kept for [`Server::stderr_tail`].
    pub stderr_ring_bytes: usize,
    /// How long a caller may wait on a full outgoing queue before the server
    /// is declared stalled and killed.
    pub writer_stall: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_message_bytes: 32 * 1024 * 1024,
            initialize_timeout: Duration::from_secs(120),
            max_pending: 64,
            stderr_ring_bytes: 64 * 1024,
            writer_stall: Duration::from_secs(5),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Spawned; `initialize` has not been answered.
    Starting,
    /// Initialized, with work-done progress or a non-quiescent server status
    /// in flight. Requests are accepted but may answer incompletely.
    Indexing {
        title: String,
        percentage: Option<u32>,
    },
    Ready,
    /// Terminal: the process is gone or was given up on.
    Exited {
        reason: String,
    },
}

/// A definition target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    /// 0-based.
    pub line: u32,
    /// 0-based, in UTF-16 code units.
    pub character: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum LspError {
    #[error("cannot start language server: {0}")]
    Spawn(#[source] std::io::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("language server request timed out")]
    Timeout,
    #[error("language server request cancelled")]
    Cancelled,
    #[error("language server exited: {0}")]
    Exited(String),
    #[error("language server protocol error: {0}")]
    Protocol(String),
    /// The server's own error; `-32801` (content modified) and `-32800`
    /// (request cancelled) are worth a retry.
    #[error("language server error {code}: {message}")]
    ResponseError { code: i64, message: String },
    #[error("too many language server requests in flight")]
    Busy,
}
