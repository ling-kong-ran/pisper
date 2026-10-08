//! Port of `packages/client/src/errors.ts` (34 lines): the client error
//! surface. Upstream models these as `Error` subclasses distinguished by
//! `name`; the port is one [`ClientError`] value tree carrying the same
//! names, messages, codes, and `cause` chains.
//!
//! Wire-visible error text (rejection `message`, hello mismatch quoting,
//! cause chains) is pinned byte-for-byte against the node oracle
//! (`tests/fixtures/client_oracle/oracle.out.txt`).

use std::borrow::Cow;
use std::fmt;

use crate::protocol::codec::ProtocolValidationError;
use crate::protocol::protocol::ProtocolError;

/// Default `DisconnectedError` message (`errors.ts:14`).
pub const CLIENT_DISCONNECTED_MESSAGE: &str = "Client is disconnected";
/// `ClientDisposedError` message (`errors.ts:22`).
pub const CLIENT_DISPOSED_MESSAGE: &str = "Client is disposed";
/// Upstream abort rejections resolve to the user's `signal.reason` or, when
/// absent, a `DOMException("The operation was aborted", "AbortError")`
/// (`client.ts:476-479`). The port's cancellation-token seam carries no
/// reason payload (disclosed seam S3), so the fixed DOMException text stands
/// in for every abort rejection.
pub const ABORTED_MESSAGE: &str = "The operation was aborted";

/// Upstream `ServerError` (`errors.ts:3-11`): a protocol error surfaced by
/// the server, with the `ProtocolErrorCode` attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerError {
    pub code: String,
    pub message: String,
}

impl ServerError {
    /// Upstream constructor: `super(error.message); this.code = error.code`.
    pub fn new(error: ProtocolError) -> ServerError {
        ServerError {
            code: error.code,
            message: error.message,
        }
    }

    pub fn name(&self) -> &'static str {
        "ServerError"
    }
}

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ServerError {}

/// Upstream `DisconnectedError` (`errors.ts:13-18`). The optional cause is
/// the port of the `{ cause }` option; `to_disconnected_error` reproduces the
/// upstream rule that the message is the cause's message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisconnectedError {
    pub message: String,
    pub cause: Option<Box<ClientError>>,
}

impl DisconnectedError {
    pub fn new() -> DisconnectedError {
        DisconnectedError {
            message: CLIENT_DISCONNECTED_MESSAGE.to_string(),
            cause: None,
        }
    }

    pub fn with_message(message: impl Into<String>) -> DisconnectedError {
        DisconnectedError {
            message: message.into(),
            cause: None,
        }
    }

    pub fn with_cause(message: impl Into<String>, cause: ClientError) -> DisconnectedError {
        DisconnectedError {
            message: message.into(),
            cause: Some(Box::new(cause)),
        }
    }

    pub fn name(&self) -> &'static str {
        "DisconnectedError"
    }
}

impl Default for DisconnectedError {
    fn default() -> DisconnectedError {
        DisconnectedError::new()
    }
}

impl fmt::Display for DisconnectedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DisconnectedError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause
            .as_deref()
            .map(|cause| cause as &(dyn std::error::Error + 'static))
    }
}

/// Upstream `ClientDisposedError` (`errors.ts:20-25`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClientDisposedError;

impl ClientDisposedError {
    pub fn message(&self) -> &'static str {
        CLIENT_DISPOSED_MESSAGE
    }

    pub fn name(&self) -> &'static str {
        "ClientDisposedError"
    }
}

impl fmt::Display for ClientDisposedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(CLIENT_DISPOSED_MESSAGE)
    }
}

impl std::error::Error for ClientDisposedError {}

/// A transport-level failure. Upstream surfaces raw `Error`s (often node IO
/// errors carrying a `code` such as `"ENOENT"`, inspected by
/// `isErrorCode`, `unix.ts:290-299`); the port carries the code explicitly so
/// the cause-chain walk stays possible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportError {
    pub message: String,
    pub code: Option<String>,
}

impl TransportError {
    pub fn new(message: impl Into<String>) -> TransportError {
        TransportError {
            message: message.into(),
            code: None,
        }
    }

    pub fn with_code(message: impl Into<String>, code: impl Into<String>) -> TransportError {
        TransportError {
            message: message.into(),
            code: Some(code.into()),
        }
    }
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TransportError {}

/// The error surface that flows through the client's promise/callback seams.
/// Upstream distributes it over `Error` subclasses (`errors.ts`) plus the
/// protocol package's `ProtocolValidationError` plus arbitrary thrown values;
/// the port is one closed enum. [`ClientError::name`] reports the upstream
/// `error.name` for every variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    /// `errors.ts:3-11`.
    Server(ServerError),
    /// `errors.ts:13-18`.
    Disconnected(DisconnectedError),
    /// `errors.ts:20-25`.
    Disposed(ClientDisposedError),
    /// `@earendil-works/pi-protocol`'s `ProtocolValidationError`.
    Protocol(ProtocolValidationError),
    /// Upstream `TypeError` texts (constructor and option validation).
    Type(String),
    /// Upstream abort rejection (`AbortError`; disclosed seam S3 — the
    /// cancellation-token seam carries no user reason).
    Aborted,
    /// Arbitrary transport/runtime failures (upstream plain `Error` / node IO
    /// errors).
    Transport(TransportError),
}

impl ClientError {
    /// The upstream `error.name` for this failure.
    pub fn name(&self) -> &'static str {
        match self {
            ClientError::Server(error) => error.name(),
            ClientError::Disconnected(error) => error.name(),
            ClientError::Disposed(error) => error.name(),
            ClientError::Protocol(_) => "ProtocolValidationError",
            ClientError::Type(_) => "TypeError",
            ClientError::Aborted => "AbortError",
            ClientError::Transport(_) => "Error",
        }
    }

    /// The upstream `error.message`.
    pub fn message(&self) -> Cow<'_, str> {
        match self {
            ClientError::Server(error) => Cow::Borrowed(&error.message),
            ClientError::Disconnected(error) => Cow::Borrowed(&error.message),
            ClientError::Disposed(error) => Cow::Borrowed(error.message()),
            ClientError::Protocol(error) => Cow::Borrowed(error.message()),
            ClientError::Type(message) => Cow::Borrowed(message),
            ClientError::Aborted => Cow::Borrowed(ABORTED_MESSAGE),
            ClientError::Transport(error) => Cow::Borrowed(&error.message),
        }
    }

    /// Port of `isErrorCode` (`unix.ts:290-299`): walk the cause chain
    /// looking for a node-style error `code`. `ServerError.code` and
    /// [`TransportError::code`] are the code-bearing nodes.
    pub fn error_code_is(&self, code: &str) -> bool {
        let mut current = Some(self);
        while let Some(error) = current {
            let node_code = match error {
                ClientError::Server(server) => Some(server.code.as_str()),
                ClientError::Transport(transport) => transport.code.as_deref(),
                _ => None,
            };
            if node_code == Some(code) {
                return true;
            }
            current = match error {
                ClientError::Disconnected(disconnected) => disconnected.cause.as_deref(),
                _ => None,
            };
        }
        false
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ClientError::Disconnected(error) => error.source(),
            _ => None,
        }
    }
}

impl From<ProtocolValidationError> for ClientError {
    fn from(error: ProtocolValidationError) -> ClientError {
        ClientError::Protocol(error)
    }
}

impl From<TransportError> for ClientError {
    fn from(error: TransportError) -> ClientError {
        ClientError::Transport(error)
    }
}

/// Port of `toDisconnectedError` (`errors.ts:31-34`): an already-disconnected
/// error passes through; anything else becomes a `DisconnectedError` whose
/// message is the cause's message and whose cause chain is preserved.
pub fn to_disconnected_error(error: ClientError) -> ClientError {
    if matches!(error, ClientError::Disconnected(_)) {
        return error;
    }
    ClientError::Disconnected(DisconnectedError::with_cause(
        error.message().into_owned(),
        error,
    ))
}
