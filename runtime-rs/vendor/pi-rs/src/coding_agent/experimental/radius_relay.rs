//! Port of upstream `experimental/radius-relay.ts`
//! (sha256 731814cce6098efde8a15bdcd92568f23400925b2c3e197871a5d80919e0a80a).
//!
//! Ported: the multiplexing envelope codec
//! ([`encode_relay_data_frame`]/[`parse_relay_data_frame`], byte-exact), the
//! host control-message validator and serializer (exact upstream JSON shape
//! and error strings), the `RadiusRelayHost` connect/retry state machine
//! (status sequence, exponential backoff 1s..30s, missing-auth 30s dwell,
//! connection multiplexing with open/close codes 1000/1012, unknown-connection
//! close, ping/pong), the client byte-transport face (binary-only messages,
//! close codes 1000/4001, error strings), the ordered-writer pending-byte
//! budget, and the `RadiusClientReconnect` reattach state machine.
//!
//! D8 seam (disclosed in this module's docs): the undici `WebSocket` event surface is the
//! [`RelayAccept`] callback face over [`RelayServerByteConnection`] /
//! [`RelayByteConnectionHandler`], pumped as [`HostInput`] / [`HostOutput`],
//! and the host/reconnect loops are driven
//! explicitly (`RadiusRelayHost::start` + `on_*` methods) instead of running
//! a self-scheduled promise loop; the live socket factory, `AbortSignal`
//! plumbing and the `bufferedAmount` drain polling stay embedder-owned (the
//! pending-byte budget that guard is for is ported as
//! [`PendingWriteBudget`]). Frames, status transitions and error strings are
//! oracle-identical (tests/fixtures/experimental_final_oracle/oracle_relay_out.json).

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::coding_agent::experimental::radius_auth::RadiusRelayAuth;
use crate::protocol::framing::DEFAULT_MAX_FRAME_LENGTH;

/// Upstream `RADIUS_RELAY_HOST_SUBPROTOCOL`.
pub const RADIUS_RELAY_HOST_SUBPROTOCOL: &str = "pi-session-relay.host.v1";
/// Upstream `RADIUS_RELAY_CLIENT_SUBPROTOCOL`.
pub const RADIUS_RELAY_CLIENT_SUBPROTOCOL: &str = "pi-session-relay.client.v1";

const RELAY_DATA_HEADER_BYTES: usize = 18;
const RELAY_DATA_FRAME_VERSION: u8 = 1;
const RELAY_DATA_FRAME_TYPE: u8 = 1;
const MAX_PENDING_BYTES: u64 = DEFAULT_MAX_FRAME_LENGTH * 4;

const HOST_RETRY_INITIAL_MS: u64 = 1_000;
const HOST_RETRY_MAX_MS: u64 = 30_000;
const MISSING_AUTH_RETRY_MS: u64 = 30_000;
const CLIENT_RETRY_INITIAL_MS: u64 = 1_000;
const CLIENT_RETRY_MAX_MS: u64 = 30_000;

/// Upstream `LOCAL_PROTOCOL_ERROR_CLOSE_CODE` (undici restricts close codes;
/// local protocol failures report as application code 4000).
pub const LOCAL_PROTOCOL_ERROR_CLOSE_CODE: u16 = 4000;
/// Upstream `LOCAL_TRANSPORT_ERROR_CLOSE_CODE`.
pub const LOCAL_TRANSPORT_ERROR_CLOSE_CODE: u16 = 4001;

/// Upstream `CONNECTION_ID_PATTERN` (canonical lowercase UUIDv4).
pub fn is_connection_id(value: &str) -> bool {
    is_uuid_v4(value)
}

fn is_uuid_v4(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (index, byte) in bytes.iter().enumerate() {
        match index {
            8 | 13 | 18 | 23 => {
                if *byte != b'-' {
                    return false;
                }
            }
            14 => {
                if *byte != b'4' {
                    return false;
                }
            }
            19 => {
                if !matches!(*byte, b'8' | b'9' | b'a' | b'b') {
                    return false;
                }
            }
            _ => {
                if !matches!(*byte, b'0'..=b'9' | b'a'..=b'f') {
                    return false;
                }
            }
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Multiplexing envelope
// ---------------------------------------------------------------------------

/// Upstream `encodeRelayDataFrame` (throws `TypeError` for a malformed id;
/// the error text is preserved).
pub fn encode_relay_data_frame(connection_id: &str, payload: &[u8]) -> Result<Vec<u8>, String> {
    if !is_connection_id(connection_id) {
        return Err("Invalid Radius relay connection ID".to_string());
    }
    let mut frame = Vec::with_capacity(RELAY_DATA_HEADER_BYTES + payload.len());
    frame.push(RELAY_DATA_FRAME_VERSION);
    frame.push(RELAY_DATA_FRAME_TYPE);
    let hex: String = connection_id.chars().filter(|c| *c != '-').collect();
    for index in 0..16 {
        let byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).unwrap_or(0);
        frame.push(byte);
    }
    frame.extend_from_slice(payload);
    Ok(frame)
}

/// Upstream `parseRelayDataFrame`'s success shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayDataFrame {
    pub connection_id: String,
    pub payload: Vec<u8>,
}

/// Upstream `parseRelayDataFrame`: `None` for short frames, wrong version or
/// type, or a decoded id that is not a canonical UUIDv4.
pub fn parse_relay_data_frame(frame: &[u8]) -> Option<RelayDataFrame> {
    if frame.len() < RELAY_DATA_HEADER_BYTES {
        return None;
    }
    if frame[0] != RELAY_DATA_FRAME_VERSION || frame[1] != RELAY_DATA_FRAME_TYPE {
        return None;
    }
    let mut hex = String::with_capacity(32);
    for byte in &frame[2..RELAY_DATA_HEADER_BYTES] {
        hex.push_str(&format!("{byte:02x}"));
    }
    let connection_id = format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    );
    if !is_connection_id(&connection_id) {
        return None;
    }
    Some(RelayDataFrame {
        connection_id,
        payload: frame[RELAY_DATA_HEADER_BYTES..].to_vec(),
    })
}

// ---------------------------------------------------------------------------
// Control messages
// ---------------------------------------------------------------------------

/// Upstream `HostInputControlMessage`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostInputControlMessage {
    Ping,
    Pong,
    ConnectionOpen {
        connection_id: String,
    },
    ConnectionClose {
        connection_id: String,
        code: Option<u16>,
    },
}

/// Upstream `HostOutputControlMessage`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostOutputControlMessage {
    Pong,
    ConnectionClose {
        connection_id: String,
        code: Option<u16>,
    },
}

impl HostOutputControlMessage {
    /// Upstream serializes with `JSON.stringify`, so keys appear in
    /// declaration order (`version`, `type`, then `connection_id`, `code`)
    /// and an absent code is omitted entirely.
    pub fn to_json(&self) -> String {
        let mut object = serde_json::Map::new();
        object.insert("version".to_string(), json!(1));
        match self {
            HostOutputControlMessage::Pong => {
                object.insert("type".to_string(), json!("pong"));
            }
            HostOutputControlMessage::ConnectionClose {
                connection_id,
                code,
            } => {
                object.insert("type".to_string(), json!("connection_close"));
                object.insert("connection_id".to_string(), json!(connection_id));
                if let Some(code) = code {
                    object.insert("code".to_string(), json!(code));
                }
            }
        }
        Value::Object(object).to_string()
    }
}

/// Upstream `parseHostControlMessage`. Exact error strings are preserved.
pub fn parse_host_control_message(value: &str) -> Result<HostInputControlMessage, String> {
    let parsed: Value = serde_json::from_str(value)
        .map_err(|_| "Invalid Radius relay control message".to_string())?;
    if !parsed.is_object() || parsed.is_array() {
        return Err("Invalid Radius relay control message".to_string());
    }
    let object = parsed.as_object().unwrap();
    let version = object.get("version");
    if version != Some(&json!(1)) {
        return Err("Unsupported Radius relay control version".to_string());
    }
    let kind = object.get("type").and_then(Value::as_str);
    match kind {
        Some("ping") => return Ok(HostInputControlMessage::Ping),
        Some("pong") => return Ok(HostInputControlMessage::Pong),
        Some("connection_open") | Some("connection_close") => {}
        _ => return Err("Invalid Radius relay control message".to_string()),
    }
    let connection_id = object.get("connection_id").and_then(Value::as_str);
    let code = object.get("code");
    let code_valid = match code {
        None | Some(Value::Null) => true,
        Some(code) => {
            code.is_u64()
                && code
                    .as_u64()
                    .is_some_and(|c| (1000..=4999).contains(&c) && c <= u16::MAX as u64)
        }
    };
    let id_valid = connection_id.is_some_and(is_connection_id);
    if id_valid && code_valid {
        let connection_id = connection_id.unwrap().to_string();
        let code = match code {
            None | Some(Value::Null) => None,
            Some(code) => Some(code.as_u64().unwrap() as u16),
        };
        return if kind == Some("connection_open") {
            Ok(HostInputControlMessage::ConnectionOpen { connection_id })
        } else {
            Ok(HostInputControlMessage::ConnectionClose {
                connection_id,
                code,
            })
        };
    }
    Err("Invalid Radius relay control message".to_string())
}

/// Upstream `relayWebSocketUrl`: `GET <gateway>/v1/session-relays/<id>/connect`
/// with http(s) rewritten to ws(s); other schemes fail with the exact text.
pub fn relay_web_socket_url(gateway: &str, server_id: &str) -> Result<String, String> {
    let url = url::Url::parse(gateway).map_err(|error| error.to_string())?;
    let mut url = url
        .join(&format!("/v1/session-relays/{server_id}/connect"))
        .map_err(|error| error.to_string())?;
    match url.scheme() {
        "https" => url
            .set_scheme("wss")
            .map_err(|_| SCHEME_ERROR_MESSAGE.to_string())?,
        "http" => url
            .set_scheme("ws")
            .map_err(|_| SCHEME_ERROR_MESSAGE.to_string())?,
        _ => {
            return Err(format!(
                "Unsupported Radius gateway protocol: {}:",
                url.scheme()
            ));
        }
    }
    Ok(url.to_string())
}

const SCHEME_ERROR_MESSAGE: &str = "Unsupported Radius gateway protocol";

/// Upstream `webSocketError`: prefer the underlying error message, then the
/// event message, then the fixed fallback.
pub fn web_socket_error(event_error: Option<&str>, event_message: Option<&str>) -> String {
    if let Some(error) = event_error {
        if !error.trim().is_empty() {
            return error.to_string();
        }
    }
    match event_message.map(str::trim) {
        Some(message) if !message.is_empty() => message.to_string(),
        _ => "Radius WebSocket connection failed".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Pending-write budget (OrderedWebSocketWriter accounting face)
// ---------------------------------------------------------------------------

/// Upstream `OrderedWebSocketWriter` byte accounting: sends are serialized,
/// tracked bytes above `MAX_PENDING_BYTES` fail with the exact error, and
/// `close()` poisons every later send.
#[derive(Debug, Default)]
pub struct PendingWriteBudget {
    pending_bytes: u64,
    closed: bool,
}

impl PendingWriteBudget {
    /// Upstream `send` size admission. Returns the exact upstream rejection
    /// strings; `Ok(())` means the write may proceed (the caller tracks it
    /// with [`PendingWriteBudget::finish`]).
    pub fn admit(&mut self, byte_length: u64) -> Result<(), String> {
        if self.closed {
            return Err("Radius relay WebSocket is closed".to_string());
        }
        if self.pending_bytes + byte_length > MAX_PENDING_BYTES {
            return Err("Radius relay exceeded its pending byte limit".to_string());
        }
        self.pending_bytes += byte_length;
        Ok(())
    }

    /// Upstream `operation.finally(() => { this.#pendingBytes -= byteLength })`.
    pub fn finish(&mut self, byte_length: u64) {
        self.pending_bytes = self.pending_bytes.saturating_sub(byte_length);
    }

    /// Upstream `close()`.
    pub fn close(&mut self) {
        self.closed = true;
    }

    /// Upstream `#closed`.
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

// ---------------------------------------------------------------------------
// Host state machine
// ---------------------------------------------------------------------------

/// Upstream `RadiusRelayHostStatus`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RadiusRelayHostStatus {
    NotAuthenticated,
    Connecting,
    Connected,
    Retrying { error: String },
}

impl RadiusRelayHostStatus {
    /// Upstream `status.status` discriminator.
    pub fn status(&self) -> &'static str {
        match self {
            RadiusRelayHostStatus::NotAuthenticated => "not_authenticated",
            RadiusRelayHostStatus::Connecting => "connecting",
            RadiusRelayHostStatus::Connected => "connected",
            RadiusRelayHostStatus::Retrying { .. } => "retrying",
        }
    }
}

/// The upstream `Server.accept` face the host drives (D8 seam: the real
/// pi-server `Server` is embedder-wired).
pub type RelayAccept =
    Arc<dyn Fn(RelayServerByteConnection) -> RelayByteConnectionHandler + Send + Sync>;

/// Upstream `RelayByteConnection`.
#[derive(Clone)]
pub struct RelayServerByteConnection {
    connection_id: String,
    closed: Arc<std::sync::atomic::AtomicBool>,
}

impl RelayServerByteConnection {
    pub fn new(connection_id: &str) -> Self {
        Self {
            connection_id: connection_id.to_string(),
            closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub fn connection_id(&self) -> &str {
        &self.connection_id
    }

    /// Upstream `get closed()`.
    pub fn closed(&self) -> bool {
        self.closed.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Upstream `markClosed`.
    pub fn mark_closed(&self) {
        self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Upstream `RelayByteConnectionHandler`.
#[derive(Default)]
pub struct RelayByteConnectionHandler {
    pub on_data: Vec<Vec<u8>>,
    pub on_close_count: usize,
    pub on_error: Vec<String>,
}

/// One outbound action the host controller emits; the embedder forwards it to
/// the live socket (D8 seam).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostOutput {
    Control(HostOutputControlMessage),
    Data {
        connection_id: String,
        payload: Vec<u8>,
    },
}

/// Events the embedder pumps into the host from the live WebSocket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostInput {
    Control(HostInputControlMessage),
    Data(Vec<u8>),
}

/// Upstream `RadiusRelayHost` state machine. The public `on_*` methods are
/// the driven form of upstream's `#run`/`#serve` loop; outputs, statuses,
/// backoff and error strings match upstream exactly.
pub struct RadiusRelayHost {
    server_id: String,
    accept: RelayAccept,
    connections: HashMap<String, RelayServerByteConnection>,
    handlers: HashMap<String, RelayByteConnectionHandler>,
    closed: bool,
    retry_ms: u64,
    /// Set while a relay connection is established (upstream `#serve` active).
    serving: bool,
    dropped_for_error: Option<String>,
}

impl RadiusRelayHost {
    /// Upstream constructor.
    pub fn new(server_id: &str, accept: RelayAccept) -> Self {
        Self {
            server_id: server_id.to_string(),
            accept,
            connections: HashMap::new(),
            handlers: HashMap::new(),
            closed: false,
            retry_ms: HOST_RETRY_INITIAL_MS,
            serving: false,
            dropped_for_error: None,
        }
    }

    pub fn server_id(&self) -> &str {
        &self.server_id
    }

    /// Upstream `start()`: the loop begins with a credential resolve and
    /// resets the backoff baseline.
    pub fn start(&mut self) {
        self.retry_ms = HOST_RETRY_INITIAL_MS;
        self.closed = false;
    }

    /// Upstream `close()`: drops every multiplexed connection (handler
    /// `onClose`, since the shutdown path passes no error).
    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.serving = false;
        self.drop_connections(None);
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    pub fn connections(&self) -> Vec<&str> {
        let mut ids: Vec<&str> = self.connections.keys().map(String::as_str).collect();
        ids.sort_unstable();
        ids
    }

    /// Upstream retry loop after `#serve` fails: double the backoff capped at
    /// `HOST_RETRY_MAX_MS`. The delay itself is embedder-owned.
    pub fn retry_delay_after_failure(&mut self) -> u64 {
        let delay = self.retry_ms;
        self.retry_ms = std::cmp::min(self.retry_ms * 2, HOST_RETRY_MAX_MS);
        delay
    }

    /// Upstream missing-auth dwell (`MISSING_AUTH_RETRY_MS`), not doubled.
    pub fn missing_auth_retry_delay(&self) -> u64 {
        MISSING_AUTH_RETRY_MS
    }

    /// Upstream `#serve` establishment: status becomes connected and the
    /// backoff resets.
    pub fn on_established(&mut self) {
        self.retry_ms = HOST_RETRY_INITIAL_MS;
        self.serving = true;
        self.dropped_for_error = None;
    }

    /// Upstream `finish()` on serve teardown: drop multiplexed connections
    /// (with `onError` when a failure caused it) and stop serving. The
    /// returned status is what `#run` would report; the embedder suppresses
    /// the emission when [`RadiusRelayHost::is_closed`] (upstream breaks the
    /// loop without reporting).
    pub fn on_disconnected(&mut self, error: Option<String>) -> RadiusRelayHostStatus {
        self.serving = false;
        let message = error.unwrap_or_else(|| "Radius relay host disconnected".to_string());
        self.drop_connections(Some(message.clone()));
        self.dropped_for_error = Some(message.clone());
        RadiusRelayHostStatus::Retrying { error: message }
    }

    /// Upstream `#handleHostMessage`. Returns the outbound actions and
    /// reports the exact close/upstream failure for protocol violations.
    pub fn handle_message(
        &mut self,
        input: HostInput,
    ) -> Result<HostHandling, RadiusRelayProtocolError> {
        let mut handling = HostHandling::default();
        match input {
            HostInput::Control(control) => match control {
                HostInputControlMessage::Ping => {
                    handling
                        .outputs
                        .push(HostOutput::Control(HostOutputControlMessage::Pong));
                }
                HostInputControlMessage::Pong => {}
                HostInputControlMessage::ConnectionOpen { connection_id } => {
                    self.open_connection(&connection_id, &mut handling);
                }
                HostInputControlMessage::ConnectionClose { connection_id, .. } => {
                    self.remote_close_connection(&connection_id);
                }
            },
            HostInput::Data(frame) => {
                let Some(parsed) = parse_relay_data_frame(&frame) else {
                    return Err(RadiusRelayProtocolError::new(
                        LOCAL_PROTOCOL_ERROR_CLOSE_CODE,
                        "Radius relay protocol error",
                        "Invalid Radius relay data frame",
                    ));
                };
                if !self.connections.contains_key(&parsed.connection_id) {
                    handling.outputs.push(HostOutput::Control(
                        HostOutputControlMessage::ConnectionClose {
                            connection_id: parsed.connection_id,
                            code: Some(1000),
                        },
                    ));
                    return Ok(handling);
                }
                if let Some(handler_slot) = self.handlers.get_mut(&parsed.connection_id) {
                    handler_slot.on_data.push(parsed.payload.clone());
                }
                handling.delivered_data.push(parsed);
            }
        }
        Ok(handling)
    }

    /// Upstream `#sendData` from the accepted server side.
    pub fn send_data(&mut self, connection_id: &str, chunk: &[u8]) -> Result<HostOutput, String> {
        if !self.connections.contains_key(connection_id) {
            return Err("Radius relay connection is closed".to_string());
        }
        Ok(HostOutput::Data {
            connection_id: connection_id.to_string(),
            payload: chunk.to_vec(),
        })
    }

    /// Upstream `RelayServerByteConnection.close`: optional final chunk, then
    /// a `connection_close` control message with code 1000.
    pub fn server_close_connection(
        &mut self,
        connection_id: &str,
        final_chunk: Option<&[u8]>,
    ) -> Result<Vec<HostOutput>, String> {
        if self.connections.remove(connection_id).is_none() {
            return Ok(Vec::new());
        }
        let mut outputs = Vec::new();
        if let Some(final_chunk) = final_chunk {
            outputs.push(HostOutput::Data {
                connection_id: connection_id.to_string(),
                payload: final_chunk.to_vec(),
            });
        }
        outputs.push(HostOutput::Control(
            HostOutputControlMessage::ConnectionClose {
                connection_id: connection_id.to_string(),
                code: Some(1000),
            },
        ));
        Ok(outputs)
    }

    /// Upstream `#openConnection`: duplicate ids fail loudly; when the
    /// accepted server already closed the relay connection the host answers
    /// `connection_close` with code 1012 instead of tracking it.
    pub fn open_connection(&mut self, connection_id: &str, handling: &mut HostHandling) {
        if self.connections.contains_key(connection_id) {
            handling.protocol_error = Some(RadiusRelayProtocolError::new(
                LOCAL_PROTOCOL_ERROR_CLOSE_CODE,
                "Radius relay protocol error",
                "Radius relay reused a connection ID",
            ));
            return;
        }
        let connection = RelayServerByteConnection::new(connection_id);
        let handler = (self.accept)(connection.clone());
        self.handlers.insert(connection_id.to_string(), handler);
        if connection.closed() {
            handling.outputs.push(HostOutput::Control(
                HostOutputControlMessage::ConnectionClose {
                    connection_id: connection_id.to_string(),
                    code: Some(1012),
                },
            ));
        } else {
            self.connections
                .insert(connection_id.to_string(), connection);
            handling.accepted.push(connection_id.to_string());
        }
    }

    /// Upstream `#remoteCloseConnection`.
    pub fn remote_close_connection(&mut self, connection_id: &str) {
        if let Some(connection) = self.connections.remove(connection_id) {
            connection.mark_closed();
            self.handlers.remove(connection_id);
        }
    }

    /// Upstream `#dropConnections`: every tracked connection is marked
    /// closed and its handler observes `onClose`, or `onError` when a
    /// failure caused the teardown.
    pub fn drop_connections(&mut self, error: Option<String>) {
        let connections: Vec<(String, RelayServerByteConnection)> =
            self.connections.drain().collect();
        for (id, connection) in connections {
            connection.mark_closed();
            if let Some(mut handler) = self.handlers.remove(&id) {
                if let Some(error) = &error {
                    handler.on_error.push(error.clone());
                } else {
                    handler.on_close_count += 1;
                }
            }
        }
    }

    /// Upstream close-code selection when the host tears down its socket.
    pub fn close_code_for_transport_error(&self) -> (u16, &'static str) {
        (LOCAL_TRANSPORT_ERROR_CLOSE_CODE, "Radius relay send failed")
    }

    /// Upstream `onClose` classification inside `#serve`.
    pub fn classify_remote_close(&self, code: u16, reason: &str) -> Option<String> {
        if code == 1000 {
            None
        } else if reason.is_empty() {
            Some(format!("Radius relay host closed ({code})"))
        } else {
            Some(format!("Radius relay host closed ({code}: {reason})"))
        }
    }

    /// Upstream `auth === undefined` status emission.
    pub fn status_for_auth(&self, auth: Option<&RadiusRelayAuth>) -> RadiusRelayHostStatus {
        match auth {
            None => RadiusRelayHostStatus::NotAuthenticated,
            Some(_) => RadiusRelayHostStatus::Connecting,
        }
    }

    /// Upstream `startServer`'s wiring face: the relay URL for one attempt.
    pub fn connect_url(&self, gateway: &str) -> Result<String, String> {
        relay_web_socket_url(gateway, &self.server_id)
    }

    /// True while `#serve` is active.
    pub fn is_serving(&self) -> bool {
        self.serving
    }

    /// The error that tore down the current serve, if any.
    pub fn dropped_for_error(&self) -> Option<&str> {
        self.dropped_for_error.as_deref()
    }
}

/// Outcomes of one delivered host message.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HostHandling {
    pub outputs: Vec<HostOutput>,
    pub accepted: Vec<String>,
    pub delivered_data: Vec<RelayDataFrame>,
    pub protocol_error: Option<RadiusRelayProtocolError>,
}

/// Upstream protocol violations close the socket with code 4000 and the
/// fixed reason, then surface the underlying error text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RadiusRelayProtocolError {
    pub close_code: u16,
    pub close_reason: &'static str,
    pub message: String,
}

impl RadiusRelayProtocolError {
    fn new(close_code: u16, close_reason: &'static str, message: impl Into<String>) -> Self {
        Self {
            close_code,
            close_reason,
            message: message.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Client byte transport
// ---------------------------------------------------------------------------

/// Upstream `RadiusClientByteTransport` state machine (driven form).
pub struct RadiusClientByteTransport {
    closed: bool,
    budget: PendingWriteBudget,
}

impl Default for RadiusClientByteTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl RadiusClientByteTransport {
    pub fn new() -> Self {
        Self {
            closed: false,
            budget: PendingWriteBudget::default(),
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Upstream `send`.
    pub fn send(&mut self, chunk_len: u64) -> Result<(), String> {
        if self.closed {
            return Err("Radius relay client is closed".to_string());
        }
        self.budget.admit(chunk_len)
    }

    /// Upstream write completion.
    pub fn finish_send(&mut self, chunk_len: u64) {
        self.budget.finish(chunk_len);
    }

    /// Upstream `close`: first close wins, socket closes with code 1000 and
    /// the fixed reason.
    pub fn close(&mut self) -> Option<(u16, &'static str)> {
        if !self.mark_closed() {
            return None;
        }
        Some((1000, "Pi client closed"))
    }

    /// Upstream non-binary message face.
    pub fn fail_non_binary(&mut self) -> TransportFailure {
        self.fail("Radius relay client received a non-binary message")
    }

    /// Upstream `#fail`.
    pub fn fail(&mut self, message: impl Into<String>) -> TransportFailure {
        let message = message.into();
        if !self.mark_closed() {
            return TransportFailure {
                close: None,
                error: None,
            };
        }
        TransportFailure {
            close: Some((
                LOCAL_TRANSPORT_ERROR_CLOSE_CODE,
                "Radius relay transport error",
            )),
            error: Some(message),
        }
    }

    /// Upstream `#markClosed`.
    pub fn mark_closed(&mut self) -> bool {
        if self.closed {
            return false;
        }
        self.closed = true;
        self.budget.close();
        true
    }
}

/// Outcome of a transport failure: the embedder closes the socket (if asked)
/// and reports the error to the client handlers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportFailure {
    pub close: Option<(u16, &'static str)>,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Client reconnect state machine
// ---------------------------------------------------------------------------

/// Upstream `RadiusClientReconnect` state machine. The embedder drives
/// connection/attachment observations into [`RadiusClientReconnect::observe`]
/// and consumes [`ReconnectAction`]s; backoff doubling matches upstream.
pub struct RadiusClientReconnect {
    desired_session_id: Option<String>,
    connected: bool,
    disposed: bool,
    reconnecting: bool,
    retry_ms: u64,
}

/// Next action for the embedder's reconnect loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconnectAction {
    /// Sleep, then call `reconnect()`.
    Retry { delay_ms: u64 },
    /// `reconnect()` succeeded; reattach the given session if present.
    Reconnected { reattach: Option<String> },
    /// Nothing to do (disposed or already connected).
    Idle,
}

impl RadiusClientReconnect {
    /// Upstream constructor: the desired session starts at the current
    /// attachment.
    pub fn new(attachment_session_id: Option<&str>) -> Self {
        Self {
            desired_session_id: attachment_session_id.map(str::to_string),
            connected: true,
            disposed: false,
            reconnecting: false,
            retry_ms: CLIENT_RETRY_INITIAL_MS,
        }
    }

    /// Upstream `onAttachmentChange`: a new attachment updates the desired
    /// session; detachment only clears it while connected.
    pub fn observe_attachment(&mut self, attachment: Option<&str>, connected: bool) {
        match attachment {
            Some(session_id) => self.desired_session_id = Some(session_id.to_string()),
            None => {
                if connected {
                    self.desired_session_id = None;
                }
            }
        }
    }

    /// Upstream `onConnectionStateChange` (`state === "disconnected"`).
    pub fn observe_disconnected(&mut self) -> ReconnectAction {
        self.connected = false;
        if self.disposed || self.reconnecting {
            return ReconnectAction::Idle;
        }
        self.reconnecting = true;
        self.retry_ms = CLIENT_RETRY_INITIAL_MS;
        ReconnectAction::Retry {
            delay_ms: CLIENT_RETRY_INITIAL_MS,
        }
    }

    /// Upstream `observe_connection_state` for `"connected"`.
    pub fn observe_connected(&mut self) {
        self.connected = true;
    }

    /// One failed `reconnect()` attempt: upstream disconnects with the error
    /// message and doubles the backoff before the next attempt
    /// (`retryMs = Math.min(retryMs * 2, CLIENT_RETRY_MAX_MS)`).
    pub fn on_reconnect_failed(&mut self, error: &str) -> ReconnectAction {
        if self.disposed {
            return ReconnectAction::Idle;
        }
        self.connected = false;
        let _ = error;
        self.retry_ms = std::cmp::min(self.retry_ms * 2, CLIENT_RETRY_MAX_MS);
        ReconnectAction::Retry {
            delay_ms: self.retry_ms,
        }
    }

    /// One successful `reconnect()` attempt: reattach the desired session.
    pub fn on_reconnected(&mut self) -> ReconnectAction {
        self.connected = true;
        self.reconnecting = false;
        ReconnectAction::Reconnected {
            reattach: self.desired_session_id.clone(),
        }
    }

    /// Upstream `dispose`: stop reconnecting; the caller disconnects an
    /// established client with the fixed reason.
    pub fn dispose(&mut self, connected: bool) -> Option<&'static str> {
        if self.disposed {
            return None;
        }
        self.disposed = true;
        self.reconnecting = false;
        if !connected {
            return Some("Radius reconnect stopped");
        }
        None
    }

    pub fn desired_session_id(&self) -> Option<&str> {
        self.desired_session_id.as_deref()
    }

    pub fn is_disposed(&self) -> bool {
        self.disposed
    }
}

// ---------------------------------------------------------------------------
// Transport factory face
// ---------------------------------------------------------------------------

/// Upstream `createRadiusClientTransportFactory` open errors when the socket
/// never reaches the selected subprotocol.
pub fn open_failure_messages() -> [String; 3] {
    [
        "Radius WebSocket connection failed".to_string(),
        "Radius relay closed before connecting (1006)".to_string(),
        "Radius relay connection cancelled".to_string(),
    ]
}

/// Upstream `openRadiusRelayWebSocket`'s subprotocol check.
pub fn unexpected_protocol_error(selected: &str) -> String {
    format!("Radius relay selected unexpected WebSocket protocol {selected:?}")
}

/// Upstream `onClose` during the opening handshake.
pub fn closed_before_connecting_error(code: u16) -> String {
    format!("Radius relay closed before connecting ({code})")
}

/// Upstream `onAbort` during the opening handshake.
pub const CONNECTION_CANCELLED_ERROR: &str = "Radius relay connection cancelled";

/// Upstream `closeWebSocket(socket, 1000, ...)` reason when opening fails.
pub const CONNECTION_FAILED_CLOSE_REASON: &str = "Radius relay connection failed";

#[cfg(test)]
mod tests;
