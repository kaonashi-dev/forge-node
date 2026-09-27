//! The handshake requires exact `protocol_version` equality.

use domain::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ClientKind {
    /// The desktop GUI.
    Gui,
    /// `forgectl` and other socket clients that must not receive terminal chatter.
    Cli,
    /// A client kind this build does not recognize (forward compatibility).
    #[serde(other)]
    Unknown,
}

/// Must be the first message after connecting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// The protocol version the client speaks; see [`crate::PROTOCOL_VERSION`].
    pub protocol_version: u32,
    /// The client's own build version, informational.
    pub client_version: String,
    pub client_kind: ClientKind,
}

impl Hello {
    /// Build a `Hello` for the given client version, defaulting to the current
    /// [`crate::PROTOCOL_VERSION`].
    #[must_use]
    pub fn new(client_version: impl Into<String>, client_kind: ClientKind) -> Self {
        Self {
            protocol_version: crate::PROTOCOL_VERSION,
            client_version: client_version.into(),
            client_kind,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloAck {
    /// The protocol version the daemon speaks.
    pub protocol_version: u32,
    /// The daemon's build version, informational.
    pub daemon_version: String,
    /// The daemon instance identifier from `daemon.lock`.
    ///
    /// Kept as a `String` (the lockfile stores it as JSON text) to avoid a
    /// `uuid` dependency in this crate; the daemon generates and formats it.
    pub instance_id: String,
    /// When this daemon instance started.
    pub started_at: Timestamp,
    /// Daemon-wide surface, needed before mounting an editor; missing means `Cells`.
    #[serde(default)]
    pub editor_surface: EditorSurface,
}

/// What an integrated editor session draws with (`[editor] surface`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum EditorSurface {
    /// A TUI under a PTY, painted by the GUI as a cell grid.
    #[default]
    Cells,
    /// A headless host publishing windows into a DOM surface.
    Dom,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloReject {
    /// The protocol version the daemon requires.
    pub daemon_protocol_version: u32,
    /// A human-readable reason, e.g. `"protocol version mismatch"`.
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_defaults_to_current_protocol_version() {
        let h = Hello::new("0.1.0", ClientKind::Gui);
        assert_eq!(h.protocol_version, crate::PROTOCOL_VERSION);
    }

    #[test]
    fn unknown_client_kind_tolerates_future_variants() {
        let back: ClientKind = serde_json::from_str("\"FutureTui\"").unwrap();
        assert_eq!(back, ClientKind::Unknown);
    }
}
