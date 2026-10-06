//! Port of `packages/server/test/unix-connection.test.ts` (65 lines, SHA256
//! `93f52cbf853ea97ab39de6b5b2b16bef65e3850bb110854aeaa5967628851da3`): the
//! `UnixByteConnection` write/close ordering.
//!
//! The upstream test drives a `ControlledSocket` that holds writes back; the
//! port uses a real `UnixStream` pair and asserts the same behavior through
//! the byte stream the peer observes: the final protocol error frame queues
//! behind the pending write (single writer task, disclosed divergence D-B)
//! and the write half shuts down only after it flushed.

use std::sync::Arc;

use tokio::io::AsyncReadExt;

use crate::protocol::codec::{encode_server_message, ServerMessageDecoder};
use crate::protocol::protocol::{ProtocolError, ServerHelloError, ServerMessage};

use crate::server::unix::UnixByteConnection;

#[tokio::test]
async fn queues_a_final_protocol_error_behind_pending_output_before_closing() {
    let (mut peer, socket) = tokio::net::UnixStream::pair().expect("socket pair");
    let connection = UnixByteConnection::new(socket, 1_000, 64 * 1024);

    let pending = connection.send(vec![1, 2, 3]);
    let final_message = ServerMessage::HelloError(ServerHelloError {
        error: ProtocolError {
            code: "invalid_request".to_string(),
            message: "Protocol violation".to_string(),
        },
    });
    let final_frame = encode_server_message(&final_message, None).unwrap();
    let closing = connection.close(Some(final_frame.clone()));

    // The close has not cut off the pending write: the final chunk queues
    // behind it on the single writer.
    pending.await.expect("pending write completes");
    closing.await.expect("graceful close completes");

    let mut observed = Vec::new();
    peer.read_to_end(&mut observed).await.expect("peer drains");
    assert!(observed.len() > 3, "peer saw no bytes");
    assert_eq!(
        &observed[..3],
        &[1, 2, 3],
        "final frame reordered ahead of pending output"
    );
    let mut decoder = ServerMessageDecoder::new(None).expect("decoder");
    let messages = decoder.push(&observed[3..]).expect("final frame decodes");
    assert_eq!(messages.len(), 1);
    match &messages[0] {
        ServerMessage::HelloError(error) => {
            assert_eq!(error.error.code, "invalid_request");
            assert_eq!(error.error.message, "Protocol violation");
        }
        other => panic!("expected the final hello_error, got {other:?}"),
    }
    drop(final_frame);

    connection.mark_closed();
    assert!(connection.closed());
}

/// Keeps the `UnixByteConnection` clone surface exercised (upstream clones
/// the connection into send/close call sites).
#[tokio::test]
async fn cloned_connection_shares_close_state() {
    let (_peer, socket) = tokio::net::UnixStream::pair().expect("socket pair");
    let connection = UnixByteConnection::new(socket, 1_000, 64 * 1024);
    let clone = connection.clone();
    assert!(!clone.closed());
    clone.mark_closed();
    assert!(connection.closed());
    let _ = Arc::new(clone);
}
