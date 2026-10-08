//! Port of `packages/server/src/testing/index.ts` (5 lines, SHA256
//! `17f9d7db7663715b71eb18625ba76f98708b8a3dddb6b6980dc2edf6f3c07064`) plus
//! the shared loopback helper and oracle reader used by the ported tests.
//!
//! The loopback pair reproduces the upstream conformance `connect()`:
//! the client's `WireChannel` feeds the server handler's `on_data`, and the
//! server `ByteConnection` feeds the client's `receive`, recording every
//! server frame (hex, including final close frames) like the oracle driver.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;

use crate::protocol::codec::encode_server_message;
use crate::protocol::protocol::{ClientMessage, ServerMessage};

use super::connection::{ByteConnection, ByteConnectionHandler};
use super::errors::OperationError;
use super::server::Server;

pub mod client;
pub mod host;
pub mod server;

pub use client::{session_call, MessagePredicate, ProtocolTestClient, SharedMessage, WireChannel};
pub use host::{create_test_server_services, Deferred, OpenGate, TestHarness, TestServerHost};
pub use server::{create_test_server, TestServer, TestServerOptions};

/// Hex-encode a frame like the oracle driver, masking embedded ASCII UUIDv4s
/// (`randomUUID` attachment ids; each text char is one byte pair).
pub fn hex_frame(bytes: &[u8]) -> String {
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    mask_uuid_hex(&hex)
}

/// Mask a UUIDv4's ASCII byte-pair form inside frame hex.
pub fn mask_uuid_hex(hex: &str) -> String {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let pattern = PATTERN.get_or_init(|| {
        regex::Regex::new(
            r"[0-9a-f]{16}2d[0-9a-f]{8}2d34[0-9a-f]{6}2d(?:38|39|61|62)[0-9a-f]{6}2d[0-9a-f]{24}",
        )
        .expect("uuid hex pattern")
    });
    pattern.replace_all(hex, "<uuid>").into_owned()
}

/// Mask a UUIDv4 in JSON text form.
pub fn mask_uuid_text(text: &str) -> String {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let pattern = PATTERN.get_or_init(|| {
        regex::Regex::new(r"[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}")
            .expect("uuid text pattern")
    });
    pattern.replace_all(text, "<uuid>").into_owned()
}

/// The recorded frames of one loopback connection (hex strings).
#[derive(Clone, Default)]
pub struct FrameLog(Arc<Mutex<Vec<String>>>);

impl FrameLog {
    pub fn push(&self, hex: String) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(hex);
    }

    pub fn frames(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn clear(&self) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}

/// The upstream conformance `connect(server)` helper: one in-memory
/// loopback pair. Returns the client plus its frame log.
pub fn connect_loopback(server: &Arc<Server>) -> (Arc<ProtocolTestClient>, FrameLog) {
    let frames = FrameLog::default();
    let handler_slot: Arc<Mutex<Option<ByteConnectionHandler>>> = Arc::new(Mutex::new(None));
    let client_closed = Arc::new(AtomicBool::new(false));

    let connection_frames = frames.clone();
    let connection_client: Arc<Mutex<Option<Arc<ProtocolTestClient>>>> = Arc::new(Mutex::new(None));
    let connection_closed = client_closed.clone();
    let connection: Arc<dyn ByteConnection> = Arc::new(LoopbackConnection {
        frames: connection_frames,
        client: connection_client.clone(),
        closed: connection_closed,
    });

    let channel_frames = frames.clone();
    let channel = Arc::new(LoopbackChannel {
        handler: handler_slot.clone(),
        connection_closed: client_closed.clone(),
        client: connection_client.clone(),
        _frames: channel_frames,
    });

    let client = Arc::new(ProtocolTestClient::new(channel));
    *connection_client.lock().unwrap() = Some(client.clone());
    let handler = server.accept(connection);
    *handler_slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(handler);
    (client, frames)
}

struct LoopbackConnection {
    frames: FrameLog,
    client: Arc<Mutex<Option<Arc<ProtocolTestClient>>>>,
    closed: Arc<AtomicBool>,
}

impl ByteConnection for LoopbackConnection {
    fn closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    fn send(&self, chunk: Vec<u8>) -> BoxFuture<'static, Result<(), OperationError>> {
        self.frames.push(hex_frame(&chunk));
        if let Some(client) = self.client.lock().unwrap().as_ref() {
            client.receive(&chunk);
        }
        Box::pin(async { Ok(()) })
    }

    fn close(
        &self,
        final_chunk: Option<Vec<u8>>,
    ) -> BoxFuture<'static, Result<(), OperationError>> {
        if let Some(final_chunk) = final_chunk {
            self.frames.push(hex_frame(&final_chunk));
            if let Some(client) = self.client.lock().unwrap().as_ref() {
                client.receive(&final_chunk);
            }
        }
        self.closed.store(true, Ordering::SeqCst);
        if let Some(client) = self.client.lock().unwrap().as_ref() {
            client.mark_closed();
        }
        Box::pin(async { Ok(()) })
    }
}

struct LoopbackChannel {
    handler: Arc<Mutex<Option<ByteConnectionHandler>>>,
    connection_closed: Arc<AtomicBool>,
    client: Arc<Mutex<Option<Arc<ProtocolTestClient>>>>,
    _frames: FrameLog,
}

impl LoopbackChannel {
    fn with_handler(&self, run: impl FnOnce(&ByteConnectionHandler)) {
        let handler = self
            .handler
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(handler) = handler.as_ref() {
            run(handler);
        }
    }
}

impl WireChannel for LoopbackChannel {
    fn send(&self, chunk: Vec<u8>) -> BoxFuture<'static, Result<(), OperationError>> {
        self.with_handler(|handler| (handler.on_data)(&chunk));
        Box::pin(async { Ok(()) })
    }

    fn send_fragmented(
        &self,
        chunk: Vec<u8>,
        split_at: usize,
    ) -> BoxFuture<'static, Result<(), OperationError>> {
        let (head, tail) = chunk.split_at(split_at.min(chunk.len()));
        self.with_handler(|handler| {
            (handler.on_data)(head);
            (handler.on_data)(tail);
        });
        Box::pin(async { Ok(()) })
    }

    fn close(&self) -> BoxFuture<'static, Result<(), OperationError>> {
        if self.connection_closed.load(Ordering::SeqCst) {
            return Box::pin(async { Ok(()) });
        }
        self.connection_closed.store(true, Ordering::SeqCst);
        self.with_handler(|handler| (handler.on_close)());
        if let Some(client) = self.client.lock().unwrap().as_ref() {
            client.mark_closed();
        }
        Box::pin(async { Ok(()) })
    }
}

/// Encodes one server message to a masked hex frame (test assertion aid).
pub fn server_frame_hex(message: &ServerMessage) -> String {
    let frame = encode_server_message(message, None).expect("test messages encode");
    hex_frame(&frame)
}

/// Encodes one client message to hex (test assertion aid).
pub fn client_frame_hex(message: &ClientMessage) -> String {
    let frame =
        crate::protocol::codec::encode_client_message(message, None).expect("test messages encode");
    hex_frame(&frame)
}

// ---------------------------------------------------------------------------
// Oracle reader
// ---------------------------------------------------------------------------

/// Loads `tests/fixtures/server_oracle/oracle.out.txt` (the captured node run of the
/// verbatim upstream sources).
pub fn oracle_lines() -> Vec<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("server_oracle")
        .join("oracle.out.txt");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("oracle output missing at {:?}: {error}", path))
        .lines()
        .map(str::to_string)
        .collect()
}

/// The raw line after the `=== <section>` marker.
pub fn oracle_line(lines: &[String], section: &str) -> String {
    let marker = format!("=== {section}");
    let start = lines
        .iter()
        .position(|line| line == &marker)
        .unwrap_or_else(|| panic!("oracle section {section} missing"));
    lines[start + 1].clone()
}

/// The parsed oracle line for one section.
pub fn oracle_section(section: &str) -> serde_json::Value {
    let line = oracle_line(&oracle_lines(), section);
    serde_json::from_str(&line)
        .unwrap_or_else(|error| panic!("oracle section {section} unparsable: {error}"))
}

/// Polls a condition for up to ~4s (upstream `expect.poll` / `vi.waitFor`).
#[cfg(test)]
pub(crate) async fn wait_until(mut condition: impl FnMut() -> bool) {
    for _ in 0..400 {
        if condition() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("condition not met in time");
}
