//! Port of `packages/server/src/errors.ts` (57 lines, SHA256
//! `1ad450b65d0fb3de78628ec38584e87adb2d5d8b360f6afdc0b6966b9c2b3fce`): the
//! server error taxonomy that can safely cross the protocol boundary.
//!
//! Upstream models one `ServerError` base class plus `Error` subclasses
//! distinguished by `name`; the port is a [`ServerError`] value carrying the
//! same `code`/`message` pairs plus named constructors. Error *texts* are
//! pinned byte-for-byte against the node oracle
//! (`tests/fixtures/server_oracle/oracle.out.txt`).
//!
//! The module also defines [`OperationError`], the closed error tree that
//! flows through the port's async seams. Upstream distributes these over
//! arbitrary thrown values (`ServerError`, chord `RemoteServiceError` and
//! `TypeError`s, protocol `ProtocolValidationError`, plain `Error`s);
//! [`OperationError::to_protocol_error`] reproduces the upstream
//! `toProtocolError` mapping (`server.ts:512-521`).

use std::fmt;

use crate::chord::services::errors::{ChordError, RemoteServiceErrorCode};

/// `INTERNAL_SERVER_ERROR_MESSAGE` (`errors.ts:15`).
pub const INTERNAL_SERVER_ERROR_MESSAGE: &str = "Internal server error";

/// Upstream `ServerError` (`errors.ts:18-27`): a host or lifecycle error that
/// can safely cross the protocol boundary. The five named upstream subclasses
/// have dedicated constructors; the generic constructor accepts any
/// `RemoteServiceErrorCode` like the upstream base class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerError {
    pub code: String,
    pub message: String,
}

impl ServerError {
    /// `new ServerError(code, message)` (`errors.ts:23-26`).
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> ServerError {
        ServerError {
            code: code.into(),
            message: message.into(),
        }
    }

    /// A code from the chord remote-service taxonomy (the
    /// `RemoteServiceErrorCode` arm of `ServerOperationErrorCode`).
    pub fn from_remote_code(
        code: RemoteServiceErrorCode,
        message: impl Into<String>,
    ) -> ServerError {
        ServerError::new(code.as_str(), message)
    }

    /// `new WrongServerError()` (`errors.ts:30-34`).
    pub fn wrong_server() -> ServerError {
        ServerError::new("wrong_server", "Request was addressed to another server")
    }

    /// `new SessionNotFoundError(message)` (`errors.ts:37-41`); the default
    /// message is `"Session was not found"`.
    pub fn session_not_found(message: Option<String>) -> ServerError {
        ServerError::new(
            "session_not_found",
            message.unwrap_or_else(|| "Session was not found".to_string()),
        )
    }

    /// `new SessionAmbiguousError()` (`errors.ts:44-48`).
    pub fn session_ambiguous() -> ServerError {
        ServerError::new(
            "session_ambiguous",
            "Session ID matches more than one session",
        )
    }

    /// `new SessionNotAttachedError()` (`errors.ts:51-55`).
    pub fn session_not_attached() -> ServerError {
        ServerError::new(
            "session_not_attached",
            "Session is not attached to this client",
        )
    }

    /// `new ServerDrainingError()` (`errors.ts:58-62`).
    pub fn server_draining() -> ServerError {
        ServerError::new("server_draining", "Server is draining")
    }
}

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ServerError {}

impl From<ChordError> for OperationError {
    fn from(error: ChordError) -> OperationError {
        OperationError::Chord(error)
    }
}

impl From<ServerError> for OperationError {
    fn from(error: ServerError) -> OperationError {
        OperationError::Server(error)
    }
}

/// The closed error tree flowing through the server module's async seams.
/// Upstream throws arbitrary values (`server.ts` / `session-router.ts`); the
/// port carries them as one enum so [`OperationError::to_protocol_error`] can
/// classify exactly like the upstream `toProtocolError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationError {
    /// Upstream `ServerError` (this module).
    Server(ServerError),
    /// Upstream chord errors: `RemoteServiceError` crosses the protocol
    /// boundary with its code; `TypeError`/`AggregateError` map to
    /// `internal_error`.
    Chord(ChordError),
    /// `@earendil-works/pi-protocol`'s `ProtocolValidationError`.
    Protocol(String),
    /// `session-router.ts`'s `SessionCleanupError` (an `AggregateError`
    /// subclass) — recognized by `SessionRouter::close`.
    SessionCleanup {
        message: String,
        errors: Vec<OperationError>,
    },
    /// Arbitrary `TypeError` / plain `Error` texts.
    Other(String),
    /// Upstream `AggregateError(errors, message)` used when several
    /// listener/session failures are collected.
    Aggregate {
        message: String,
        errors: Vec<OperationError>,
    },
}

impl OperationError {
    /// The upstream `error.message`.
    pub fn message(&self) -> &str {
        match self {
            OperationError::Server(error) => &error.message,
            OperationError::Chord(error) => error.message(),
            OperationError::Protocol(message) | OperationError::Other(message) => message,
            OperationError::SessionCleanup { message, .. }
            | OperationError::Aggregate { message, .. } => message,
        }
    }

    /// The upstream `error.name` (test/debug surface).
    pub fn name(&self) -> &'static str {
        match self {
            OperationError::Server(_) => "ServerError",
            OperationError::Chord(_) => "Error",
            OperationError::Protocol(_) => "ProtocolValidationError",
            OperationError::SessionCleanup { .. } => "AggregateError",
            OperationError::Other(_) => "Error",
            OperationError::Aggregate { .. } => "AggregateError",
        }
    }

    /// Whether the upstream `toProtocolError` would take the
    /// "everything else" branch (`reportError` + `internal_error`): true for
    /// every value that is not a `ServerError`, chord `RemoteServiceError`,
    /// or `ProtocolValidationError`.
    pub fn reports_internal(&self) -> bool {
        match self {
            OperationError::Server(_) | OperationError::Protocol(_) => false,
            OperationError::Chord(ChordError::Remote { .. }) => false,
            OperationError::Chord(ChordError::Type(_))
            | OperationError::Chord(ChordError::Aggregate { .. })
            | OperationError::SessionCleanup { .. }
            | OperationError::Other(_)
            | OperationError::Aggregate { .. } => true,
        }
    }

    /// Port of `toProtocolError` (`server.ts:512-521`): `ServerError` and
    /// chord `RemoteServiceError` cross with their code; protocol validation
    /// errors become `invalid_request`; everything else is reported (by the
    /// caller, which owns the error observer) as `internal_error` with the
    /// fixed `INTERNAL_SERVER_ERROR_MESSAGE`.
    pub fn to_protocol_error(&self) -> crate::protocol::protocol::ProtocolError {
        use crate::protocol::protocol::ProtocolError;
        match self {
            OperationError::Server(error) => ProtocolError {
                code: error.code.clone(),
                message: error.message.clone(),
            },
            OperationError::Chord(ChordError::Remote { code, message }) => ProtocolError {
                code: code.as_str().to_string(),
                message: message.clone(),
            },
            OperationError::Protocol(message) => ProtocolError {
                code: "invalid_request".to_string(),
                message: message.clone(),
            },
            OperationError::SessionCleanup { .. }
            | OperationError::Chord(ChordError::Type(_))
            | OperationError::Chord(ChordError::Aggregate { .. })
            | OperationError::Other(_)
            | OperationError::Aggregate { .. } => ProtocolError {
                code: "internal_error".to_string(),
                message: INTERNAL_SERVER_ERROR_MESSAGE.to_string(),
            },
        }
    }
}

impl fmt::Display for OperationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for OperationError {}

/// The shutdown failure surfaced by `Server::close` / `Server::closed`.
/// Upstream rethrows a single error verbatim or wraps several in an
/// `AggregateError(errors, message)`; the port keeps both shapes and the
/// aggregation happens at the call sites (`server.ts:197-204`,
/// `server.ts:499-501`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShutdownError {
    /// One upstream error rethrown verbatim.
    Single(OperationError),
    /// Upstream `new AggregateError(errors, message)`.
    Aggregate {
        message: String,
        errors: Vec<OperationError>,
    },
}

impl ShutdownError {
    /// The upstream `error.message`.
    pub fn message(&self) -> &str {
        match self {
            ShutdownError::Single(error) => error.message(),
            ShutdownError::Aggregate { message, .. } => message,
        }
    }
}

impl fmt::Display for ShutdownError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for ShutdownError {}
