//! Port of the observable surface of `packages/client/src/errors.ts`:
//! names, messages, and the cause-chain walk (`unix.ts:290-299`
//! `isErrorCode` semantics).

use crate::client::errors::{
    to_disconnected_error, ClientDisposedError, ClientError, DisconnectedError, ServerError,
    TransportError, ABORTED_MESSAGE, CLIENT_DISCONNECTED_MESSAGE, CLIENT_DISPOSED_MESSAGE,
};
use crate::protocol::codec::ProtocolValidationError;

#[test]
fn server_error_carries_code_and_message() {
    let error = ServerError::new(crate::protocol::protocol::ProtocolError {
        code: "version".to_string(),
        message: "Unsupported protocol version".to_string(),
    });
    assert_eq!(error.name(), "ServerError");
    assert_eq!(error.code, "version");
    assert_eq!(error.to_string(), "Unsupported protocol version");
}

#[test]
fn disconnected_error_default_message() {
    let error = DisconnectedError::new();
    assert_eq!(error.name(), "DisconnectedError");
    assert_eq!(error.message, CLIENT_DISCONNECTED_MESSAGE);
}

#[test]
fn to_disconnected_error_passes_through_and_wraps() {
    let already =
        ClientError::Disconnected(DisconnectedError::with_message("Byte transport closed"));
    assert_eq!(to_disconnected_error(already.clone()), already);

    let wrapped = to_disconnected_error(transport_error("read failed"));
    let ClientError::Disconnected(disconnected) = &wrapped else {
        panic!("wrapped variant");
    };
    assert_eq!(disconnected.message, "read failed");
    assert_eq!(
        disconnected
            .cause
            .as_deref()
            .map(ClientError::message)
            .as_deref(),
        Some("read failed")
    );
}

#[test]
fn disposed_error_surface() {
    let error = ClientDisposedError;
    assert_eq!(error.name(), "ClientDisposedError");
    assert_eq!(error.message(), CLIENT_DISPOSED_MESSAGE);
    let client_error = ClientError::Disposed(error);
    assert_eq!(client_error.name(), "ClientDisposedError");
    assert_eq!(client_error.message(), CLIENT_DISPOSED_MESSAGE);
}

#[test]
fn aborted_error_uses_the_upstream_domexception_text() {
    assert_eq!(ClientError::Aborted.name(), "AbortError");
    assert_eq!(ClientError::Aborted.message(), ABORTED_MESSAGE);
}

#[test]
fn error_code_walks_the_cause_chain() {
    // `DisconnectedError(cause: TransportError(code: ENOENT))` resolves the
    // code two nodes down, exactly like `isErrorCode`'s `current.cause` walk.
    let error = to_disconnected_error(ClientError::Transport(TransportError::with_code(
        "connect ENOENT",
        "ENOENT",
    )));
    assert!(error.error_code_is("ENOENT"));
    assert!(!error.error_code_is("ECONNREFUSED"));

    // A code-bearing node that does not match still lets the walk continue.
    let server_first = ClientError::Disconnected(DisconnectedError::with_cause(
        "wrapped",
        ClientError::Transport(TransportError::with_code("io", "ETIMEDOUT")),
    ));
    assert!(server_first.error_code_is("ETIMEDOUT"));

    // ServerError.code participates like upstream's `"code" in error`.
    let server = ClientError::Server(ServerError::new(crate::protocol::protocol::ProtocolError {
        code: "version".to_string(),
        message: "no".to_string(),
    }));
    assert!(server.error_code_is("version"));

    let plain = ClientError::Protocol(ProtocolValidationError::new("nope"));
    assert!(!plain.error_code_is("ENOENT"));
}

fn transport_error(message: &str) -> ClientError {
    ClientError::Transport(TransportError::new(message))
}
