//! The connection handshake (§9.2).
//!
//! A client opens with [`Hello`]; the daemon replies with either [`HelloAck`]
//! (versions match) or [`HelloReject`] (they do not). In the MVP the daemon
//! requires `protocol_version` equality; N/N-1 compatibility is deferred (§9.2).

use domain::Timestamp;
use serde::{Deserialize, Serialize};

/// What kind of client is connecting. Only the GUI exists in the MVP; the
/// debug CLI (`dump --json`, §10.4) also connects as [`ClientKind::Gui`] for now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ClientKind {
    /// The desktop GUI.
    Gui,
    /// A client kind this build does not recognize (forward compatibility).
    #[serde(other)]
    Unknown,
}

/// First message a client sends after connecting (§9.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// The protocol version the client speaks; see [`crate::PROTOCOL_VERSION`].
    pub protocol_version: u32,
    /// The client's own build version, informational.
    pub client_version: String,
    /// The kind of client connecting.
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

/// Sent by the daemon when the handshake succeeds (§9.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloAck {
    /// The protocol version the daemon speaks.
    pub protocol_version: u32,
    /// The daemon's build version, informational.
    pub daemon_version: String,
    /// The daemon instance identifier from `daemon.lock` (§9.2).
    ///
    /// Kept as a `String` (the lockfile stores it as JSON text) to avoid a
    /// `uuid` dependency in this crate; the daemon generates and formats it.
    pub instance_id: String,
    /// When this daemon instance started.
    pub started_at: Timestamp,
}

/// Sent by the daemon when the handshake fails on version mismatch (§9.2).
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
