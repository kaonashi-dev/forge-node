//! Protocol-level errors carried in `DaemonMessage::Response`.
//!
//! [`ProtocolError`] is the *wire* error a request can fail with; it is distinct
//! from [`crate::framing::ProtocolCodecError`], which is the *local* codec error
//! raised while framing bytes on either side.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Machine-readable error category.
///
/// The variants mirror the plan exactly. `Unknown` is the forward-compatible
/// catch-all: an older peer decoding a code introduced by a newer peer maps it
/// here instead of failing to decode the whole message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ErrorCode {
    /// The request was malformed or referenced impossible arguments.
    InvalidRequest,
    /// A referenced entity (project/workspace/session/terminal) does not exist.
    NotFound,
    /// The request conflicts with existing state (e.g. duplicate project path).
    Conflict,
    /// A precondition was not met, e.g. `CloseSession` on a `Running` session.
    PreconditionFailed,
    /// A `git` CLI invocation failed (§ADR-008).
    GitError,
    /// Spawning a PTY/process failed (binary missing, cwd invalid, ...).
    SpawnError,
    /// An underlying I/O operation failed.
    IoError,
    /// The requested agent provider is not installed (§13.1).
    ProviderNotInstalled,
    /// The peer violated the protocol (bad sequence, unexpected message).
    ProtocolViolation,
    /// An unclassified internal error.
    Internal,
    /// A code this build does not recognize (forward compatibility).
    #[serde(other)]
    Unknown,
}

/// A structured error returned when a [`crate::request::Request`] fails.
///
/// Marked `#[non_exhaustive]` so fields can be added later; construct it through
/// the provided constructors rather than a struct literal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ProtocolError {
    /// The machine-readable category.
    pub code: ErrorCode,
    /// A human-readable, developer-facing message. Not localized.
    pub message: String,
    /// Optional extra context (a git stderr tail, a path, ...).
    pub details: Option<String>,
}

impl ProtocolError {
    /// Build an error with a `code` and `message` and no `details`.
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
        }
    }

    /// Build an error with `details` attached.
    #[must_use]
    pub fn with_details(
        code: ErrorCode,
        message: impl Into<String>,
        details: impl Into<String>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            details: Some(details.into()),
        }
    }

    /// Convenience constructor for [`ErrorCode::InvalidRequest`].
    #[must_use]
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidRequest, message)
    }

    /// Convenience constructor for [`ErrorCode::NotFound`].
    #[must_use]
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    /// Convenience constructor for [`ErrorCode::Conflict`].
    #[must_use]
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Conflict, message)
    }

    /// Convenience constructor for [`ErrorCode::PreconditionFailed`].
    #[must_use]
    pub fn precondition_failed(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::PreconditionFailed, message)
    }

    /// Convenience constructor for [`ErrorCode::Internal`].
    #[must_use]
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)?;
        if let Some(details) = &self.details {
            write!(f, " ({details})")?;
        }
        Ok(())
    }
}

impl std::error::Error for ProtocolError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_serializes_as_its_name() {
        let json = serde_json::to_string(&ErrorCode::NotFound).unwrap();
        assert_eq!(json, "\"NotFound\"");
    }

    #[test]
    fn unknown_error_code_tolerates_future_variants() {
        // A code from a newer peer must decode to `Unknown`, not fail.
        let back: ErrorCode = serde_json::from_str("\"SomeFutureCode\"").unwrap();
        assert_eq!(back, ErrorCode::Unknown);
    }

    #[test]
    fn constructors_set_fields() {
        let e = ProtocolError::with_details(ErrorCode::GitError, "boom", "stderr");
        assert_eq!(e.code, ErrorCode::GitError);
        assert_eq!(e.message, "boom");
        assert_eq!(e.details.as_deref(), Some("stderr"));
        assert_eq!(ProtocolError::not_found("x").code, ErrorCode::NotFound);
    }
}
