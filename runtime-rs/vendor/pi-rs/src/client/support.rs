//! Test support. Port of `packages/client/test/support.ts` (84 lines,
//! SHA256 `9864826d3bcb4812efe522b004bc9aa83bd4c04f1636d512b034b036e0eb2cd6`):
//! an in-memory byte server that performs the server side of the handshake
//! over fake transports.
//!
//! Also carries the oracle reader (`tests/fixtures/client_oracle/oracle.out.txt`)
//! and an order-preserving JSON parser so assertions compare delivered JSON
//! values against `JSON.stringify` output byte-for-byte (serde_json would
//! sort object keys).

use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};

use futures::future::BoxFuture;
use tokio::sync::watch;

use crate::protocol::codec::ClientMessageDecoder;
use crate::protocol::json::JsonValue;
use crate::protocol::protocol::{ClientHello, ClientMessage, ServerHello, ServerMessage};

use super::errors::ClientError;
use super::transport::{ByteTransport, ByteTransportFactory, ByteTransportHandlers};

pub(crate) const SERVER_ID: &str = "00000000-0000-4000-8000-000000000001";
pub(crate) const OTHER_SERVER_ID: &str = "00000000-0000-4000-8000-000000000002";

struct MemoryServerState {
    messages: Vec<ClientMessage>,
    handlers: Option<(u64, ByteTransportHandlers)>,
    decoder: ClientMessageDecoder,
    client_close_count: usize,
}

/// Upstream `MemoryByteServer`.
pub(crate) struct MemoryByteServer {
    server_id: String,
    state: Mutex<MemoryServerState>,
    message_count: watch::Sender<usize>,
    /// Held so the watch channel never closes between waits.
    _message_count_rx: watch::Receiver<usize>,
    next_transport_id: AtomicU64,
}

impl MemoryByteServer {
    pub(crate) fn new() -> Arc<MemoryByteServer> {
        Self::with_server_id(SERVER_ID)
    }

    pub(crate) fn with_server_id(server_id: &str) -> Arc<MemoryByteServer> {
        let (message_count, message_count_rx) = watch::channel(0);
        Arc::new(MemoryByteServer {
            server_id: server_id.to_string(),
            state: Mutex::new(MemoryServerState {
                messages: Vec::new(),
                handlers: None,
                decoder: ClientMessageDecoder::new(None).expect("default decoder"),
                client_close_count: 0,
            }),
            message_count,
            _message_count_rx: message_count_rx,
            next_transport_id: AtomicU64::new(0),
        })
    }

    /// `support.ts:22-47`.
    pub(crate) fn transport_factory(self: &Arc<Self>) -> ByteTransportFactory {
        let server = self.clone();
        Arc::new(move |handlers: ByteTransportHandlers| {
            let transport = server.connect(handlers);
            Box::pin(async move { Ok(transport) })
        })
    }

    fn connect(self: &Arc<Self>, handlers: ByteTransportHandlers) -> Arc<dyn ByteTransport> {
        let transport_id = self.next_transport_id.fetch_add(1, Ordering::SeqCst);
        {
            let mut state = self.state.lock().unwrap();
            state.handlers = Some((transport_id, handlers));
            state.decoder = ClientMessageDecoder::new(None).expect("default decoder");
        }
        Arc::new(MemoryTransport {
            server: self.clone(),
            transport_id,
            closed: AtomicBool::new(false),
        })
    }

    fn ingest(&self, chunk: &[u8]) {
        let replies = {
            let mut state = self.state.lock().unwrap();
            let messages = match state.decoder.push(chunk) {
                Ok(messages) => messages,
                Err(error) => {
                    // Upstream lets the promise rejection bubble to the
                    // connection, which fails it; the tests never send
                    // invalid client frames.
                    panic!("memory server decode failed: {}", error.message());
                }
            };
            let mut replies = Vec::new();
            for message in messages {
                if let ClientMessage::Hello(ClientHello { .. }) = &message {
                    replies.push(ServerMessage::Hello(ServerHello {
                        server_id: self.server_id.clone(),
                    }));
                }
                state.messages.push(message);
            }
            self.message_count.send_replace(state.messages.len());
            replies
        };
        for reply in replies {
            self.send(&reply);
        }
    }

    /// `support.ts:49-52`.
    pub(crate) async fn wait_for_messages(&self, count: usize) {
        let mut receiver = self.message_count.subscribe();
        loop {
            if *receiver.borrow_and_update() >= count {
                return;
            }
            if receiver.changed().await.is_err() {
                // Sender lives as long as the server; unreachable in tests.
                return;
            }
        }
    }

    pub(crate) fn messages(&self) -> Vec<ClientMessage> {
        self.state.lock().unwrap().messages.clone()
    }

    /// `support.ts:54-57`.
    pub(crate) fn send(&self, message: &ServerMessage) {
        let handlers = {
            let state = self.state.lock().unwrap();
            state
                .handlers
                .as_ref()
                .map(|(_, handlers)| handlers.clone())
        };
        let frame = crate::protocol::codec::encode_server_message(message, None)
            .expect("memory server sends valid messages");
        let handlers = handlers.expect("No client connection");
        (handlers.on_data)(&frame);
    }

    /// `support.ts:59-62`.
    pub(crate) fn send_raw(&self, chunk: &[u8]) {
        let handlers = {
            let state = self.state.lock().unwrap();
            state
                .handlers
                .as_ref()
                .map(|(_, handlers)| handlers.clone())
        };
        let handlers = handlers.expect("No client connection");
        (handlers.on_data)(chunk);
    }

    /// `support.ts:64-68`.
    pub(crate) fn disconnect(&self) {
        let handlers = self.take_handlers();
        if let Some((_, handlers)) = handlers {
            (handlers.on_close)();
        }
    }

    /// `support.ts:70-74`.
    pub(crate) fn error(&self, error: ClientError) {
        let handlers = self.take_handlers();
        if let Some((_, handlers)) = handlers {
            (handlers.on_error)(error);
        }
    }

    fn take_handlers(&self) -> Option<(u64, ByteTransportHandlers)> {
        self.state.lock().unwrap().handlers.take()
    }

    pub(crate) fn client_close_count(&self) -> usize {
        self.state.lock().unwrap().client_close_count
    }

    fn close_transport(&self, transport_id: u64) {
        let mut state = self.state.lock().unwrap();
        let matches_current = state
            .handlers
            .as_ref()
            .is_some_and(|(id, _)| *id == transport_id);
        if matches_current {
            state.handlers = None;
        }
        state.client_close_count += 1;
    }
}

struct MemoryTransport {
    server: Arc<MemoryByteServer>,
    transport_id: u64,
    closed: AtomicBool,
}

impl ByteTransport for MemoryTransport {
    fn send(&self, chunk: Vec<u8>) -> BoxFuture<'static, Result<(), ClientError>> {
        let server = self.server.clone();
        Box::pin(async move {
            server.ingest(&chunk);
            Ok(())
        })
    }

    fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.server.close_transport(self.transport_id);
    }
}

/// A factory wrapper that hex-records every client->server chunk before the
/// verbatim server handling runs (oracle.mjs `recordingFactory`).
pub(crate) fn recording_factory(
    server: &Arc<MemoryByteServer>,
    log: Arc<Mutex<Vec<String>>>,
) -> ByteTransportFactory {
    let inner = server.transport_factory();
    Arc::new(move |handlers: ByteTransportHandlers| {
        let log = log.clone();
        let inner = inner.clone();
        Box::pin(async move {
            let transport = (inner)(handlers).await?;
            Ok(Arc::new(RecordingTransport { transport, log }) as Arc<dyn ByteTransport>)
        })
    })
}

struct RecordingTransport {
    transport: Arc<dyn ByteTransport>,
    log: Arc<Mutex<Vec<String>>>,
}

impl ByteTransport for RecordingTransport {
    fn send(&self, chunk: Vec<u8>) -> BoxFuture<'static, Result<(), ClientError>> {
        self.log.lock().unwrap().push(hex(&chunk));
        let transport = self.transport.clone();
        Box::pin(async move { transport.send(chunk).await })
    }

    fn close(&self) {
        self.transport.close();
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

// ---------------------------------------------------------------------------
// Oracle reader
// ---------------------------------------------------------------------------

/// Loads `tests/fixtures/client_oracle/oracle.out.txt` (the captured node run of the
/// verbatim upstream sources).
pub(crate) fn oracle_lines() -> Vec<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("client_oracle")
        .join("oracle.out.txt");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("oracle output missing at {:?}: {error}", path))
        .lines()
        .map(str::to_string)
        .collect()
}

/// The line `offset` lines after the `=== <section>` marker.
pub(crate) fn oracle_line(lines: &[String], section: &str, offset: usize) -> String {
    let marker = format!("=== {section}");
    let start = lines
        .iter()
        .position(|line| line == &marker)
        .unwrap_or_else(|| panic!("oracle section {section} missing"));
    lines[start + 1 + offset]
        .trim()
        .trim_start_matches(char::is_whitespace)
        .to_string()
}

/// Parses one `JSON.stringify` line into the ordered JSON value tree, so
/// object key order survives for exact-value comparison (serde_json would
/// sort keys).
pub(crate) fn parse_ordered_json(text: &str) -> JsonValue {
    let bytes: Vec<char> = text.chars().collect();
    let mut position = 0;
    let value = parse_json_value(&bytes, &mut position);
    skip_ws(&bytes, &mut position);
    assert_eq!(position, bytes.len(), "trailing JSON input: {text}");
    value
}

fn skip_ws(input: &[char], position: &mut usize) {
    while *position < input.len() && input[*position].is_whitespace() {
        *position += 1;
    }
}

fn parse_json_value(input: &[char], position: &mut usize) -> JsonValue {
    skip_ws(input, position);
    match input[*position] {
        '{' => parse_json_object(input, position),
        '[' => parse_json_array(input, position),
        '"' => JsonValue::String(parse_json_string(input, position)),
        't' => {
            expect(input, position, "true");
            JsonValue::Bool(true)
        }
        'f' => {
            expect(input, position, "false");
            JsonValue::Bool(false)
        }
        'n' => {
            expect(input, position, "null");
            JsonValue::Null
        }
        _ => parse_json_number(input, position),
    }
}

fn expect(input: &[char], position: &mut usize, literal: &str) {
    for expected in literal.chars() {
        assert_eq!(input[*position], expected, "bad JSON literal");
        *position += 1;
    }
}

fn parse_json_object(input: &[char], position: &mut usize) -> JsonValue {
    expect(input, position, "{");
    let mut entries: Vec<(String, JsonValue)> = Vec::new();
    skip_ws(input, position);
    if input[*position] == '}' {
        *position += 1;
        return JsonValue::Object(entries);
    }
    loop {
        skip_ws(input, position);
        let key = parse_json_string(input, position);
        skip_ws(input, position);
        expect(input, position, ":");
        entries.push((key, parse_json_value(input, position)));
        skip_ws(input, position);
        match input[*position] {
            ',' => *position += 1,
            '}' => {
                *position += 1;
                return JsonValue::Object(entries);
            }
            other => panic!("unexpected JSON object character {other}"),
        }
    }
}

fn parse_json_array(input: &[char], position: &mut usize) -> JsonValue {
    expect(input, position, "[");
    let mut items = Vec::new();
    skip_ws(input, position);
    if input[*position] == ']' {
        *position += 1;
        return JsonValue::Array(items);
    }
    loop {
        items.push(parse_json_value(input, position));
        skip_ws(input, position);
        match input[*position] {
            ',' => *position += 1,
            ']' => {
                *position += 1;
                return JsonValue::Array(items);
            }
            other => panic!("unexpected JSON array character {other}"),
        }
    }
}

fn parse_json_string(input: &[char], position: &mut usize) -> String {
    expect(input, position, "\"");
    let mut out = String::new();
    while input[*position] != '"' {
        if input[*position] == '\\' {
            *position += 1;
            match input[*position] {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'b' => out.push('\u{08}'),
                'f' => out.push('\u{0c}'),
                'u' => {
                    let hex_text: String = input[*position + 1..*position + 5].iter().collect();
                    let code = u32::from_str_radix(&hex_text, 16).expect("json \\u escape");
                    *position += 4;
                    if (0xD800..0xDC00).contains(&code)
                        && input[*position + 1] == '\\'
                        && input[*position + 2] == 'u'
                    {
                        let low_hex: String = input[*position + 3..*position + 7].iter().collect();
                        let low = u32::from_str_radix(&low_hex, 16).expect("json \\u escape");
                        *position += 6;
                        let combined = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                        out.push(char::from_u32(combined).expect("surrogate pair"));
                    } else {
                        out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                    }
                }
                other => panic!("unknown JSON escape \\{other}"),
            }
            *position += 1;
        } else {
            out.push(input[*position]);
            *position += 1;
        }
    }
    *position += 1;
    out
}

fn parse_json_number(input: &[char], position: &mut usize) -> JsonValue {
    let start = *position;
    while *position < input.len()
        && (input[*position].is_ascii_digit()
            || matches!(input[*position], '-' | '+' | '.' | 'e' | 'E'))
    {
        *position += 1;
    }
    let text: String = input[start..*position].iter().collect();
    if let Ok(value) = text.parse::<u64>() {
        return JsonValue::Number(crate::protocol::json::Number::Uint(value));
    }
    if let Ok(value) = text.parse::<i64>() {
        return JsonValue::Number(crate::protocol::json::Number::Int(value));
    }
    JsonValue::Number(crate::protocol::json::Number::Float(
        text.parse().expect("json number"),
    ))
}
