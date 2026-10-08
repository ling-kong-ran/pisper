//! Streamable HTTP transport, ported from upstream
//! `packages/mcp/src/transports/streamable-http.ts`: JSON-RPC over HTTP POST
//! with `application/json` or `text/event-stream` responses, the
//! server-to-client GET SSE stream, session and protocol-version headers,
//! bearer-token auth with a one-shot 401/403 retry through an auth provider,
//! and SSE stream resumption with `Last-Event-ID` plus exponential backoff.
//!
//! Port notes (disclosed divergences):
//! - Upstream retries errors with `code` strings starting with `E`/`UND_ERR`
//!   (undici internals); the port classifies fetch failures through
//!   [`crate::mcp::auth_provider::FetchError::network`] (the upstream
//!   `TypeError` analog) and treats every other non-HTTP error as
//!   non-retryable.
//! - The abort controller maps to a `CancellationToken`: reconnect sleeps
//!   return early on close, and in-flight fetches observe the token.
//! - `Date.now()` in the GET-stream health check maps to
//!   `std::time::Instant::now()` deltas.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use futures::StreamExt;
use serde::Serialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::mcp::auth_provider::{
    default_fetch, AuthProvider, ByteStream, FetchRequest, FetchResponse, McpFetch,
    UnauthorizedContext,
};
use crate::mcp::protocol::jsonrpc::{
    is_json_rpc_response, parse_json_rpc_message, JsonRpcId, JsonRpcMessage, JsonRpcResponse,
    McpAuthRequiredError, McpClientError, McpHttpError, McpSessionExpiredError,
    JSON_RPC_ERROR_CODES_INTERNAL_ERROR,
};
use crate::mcp::transports::{
    McpTransport, TransportCloseListener, TransportErrorListener, TransportEvents,
    TransportMessageListener, Unsubscribe, DEFAULT_MAX_MESSAGE_BYTES,
};

const MAX_ERROR_BODY_BYTES: usize = 8 * 1024;
const ERROR_MESSAGE_BODY_CHARS: usize = 500;
const DEFAULT_RECONNECT_INITIAL_DELAY_MS: u64 = 1_000;
const DEFAULT_RECONNECT_MAX_DELAY_MS: u64 = 30_000;
const DEFAULT_RECONNECT_MAX_RETRIES: u64 = 5;

/// Upstream `SseEvent`. Serialization order matches the upstream object
/// spread: `{event?, data, id?}`.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct SseEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    pub data: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// An SSE event callback.
pub type SseEventCallback<'a> = Box<dyn FnMut(SseEvent) + Send + 'a>;

/// The `id`-field callback.
pub type SseIdCallback<'a> = Box<dyn FnMut(&str) + Send + 'a>;

/// The `retry`-field callback.
pub type SseRetryCallback<'a> = Box<dyn FnMut(u64) + Send + 'a>;

/// Upstream `ConsumeSseOptions`. The callbacks borrow for the duration of
/// the `consume_sse_stream` call.
pub struct ConsumeSseOptions<'a> {
    pub max_event_bytes: Option<usize>,
    pub on_event: SseEventCallback<'a>,
    /// Called for every `id` field, including events without data (for
    /// example resumption priming events).
    pub on_id: Option<SseIdCallback<'a>>,
    /// Called for every valid `retry` field, in milliseconds.
    pub on_retry: Option<SseRetryCallback<'a>>,
}

impl<'a> Default for ConsumeSseOptions<'a> {
    fn default() -> Self {
        ConsumeSseOptions {
            max_event_bytes: None,
            on_event: Box::new(|_| {}),
            on_id: None,
            on_retry: None,
        }
    }
}

/// Upstream `consumeSseStream`: incremental `text/event-stream` parsing over
/// a byte stream, with the upstream byte budgets and BOM handling
/// (`TextDecoder` strips one leading U+FEFF).
pub async fn consume_sse_stream(
    body: ByteStream,
    options: ConsumeSseOptions<'_>,
) -> Result<(), McpClientError> {
    SseParser::new(options).run(body).await
}

struct SseParser<'a> {
    options: ConsumeSseOptions<'a>,
    max_event_bytes: usize,
    buffered: String,
    pending_bytes: Vec<u8>,
    bom_stripped: bool,
    event_name: Option<String>,
    event_id: Option<String>,
    data_lines: Vec<String>,
    /// Bytes of the pending event's data, including the "\n" joins, so
    /// events streamed as many short `data:` lines without a terminating
    /// blank line cannot grow without bound.
    data_bytes: usize,
}

impl<'a> SseParser<'a> {
    fn new(options: ConsumeSseOptions<'a>) -> Self {
        let max_event_bytes = options.max_event_bytes.unwrap_or(DEFAULT_MAX_MESSAGE_BYTES);
        SseParser {
            options,
            max_event_bytes,
            buffered: String::new(),
            pending_bytes: Vec::new(),
            bom_stripped: false,
            event_name: None,
            event_id: None,
            data_lines: Vec::new(),
            data_bytes: 0,
        }
    }

    fn decode_chunk(&mut self, chunk: &[u8]) {
        // `decoder.decode(value, { stream: true })`: incomplete trailing
        // sequences stay buffered.
        self.pending_bytes.extend_from_slice(chunk);
        let valid_up_to = match std::str::from_utf8(&self.pending_bytes) {
            Ok(_) => self.pending_bytes.len(),
            Err(error) => error.valid_up_to(),
        };
        let decoded = String::from_utf8_lossy(&self.pending_bytes[..valid_up_to]).into_owned();
        self.pending_bytes.drain(..valid_up_to);
        if !self.bom_stripped && !decoded.is_empty() {
            self.bom_stripped = true;
            let stripped = decoded.strip_prefix('\u{FEFF}').unwrap_or(decoded.as_str());
            self.buffered.push_str(stripped);
        } else {
            self.buffered.push_str(&decoded);
        }
    }

    fn flush_decoder(&mut self) {
        if !self.pending_bytes.is_empty() {
            let decoded = String::from_utf8_lossy(&self.pending_bytes).into_owned();
            self.pending_bytes.clear();
            if !self.bom_stripped {
                self.bom_stripped = true;
                let stripped = decoded.strip_prefix('\u{FEFF}').unwrap_or(decoded.as_str());
                self.buffered.push_str(stripped);
            } else {
                self.buffered.push_str(&decoded);
            }
        }
    }

    /// Upstream `dispatch`: emit the pending event or reset the priming
    /// fields.
    fn dispatch(&mut self) {
        if self.data_lines.is_empty() {
            self.event_name = None;
            self.event_id = None;
            return;
        }
        let data = self.data_lines.join("\n");
        let event = SseEvent {
            event: self.event_name.take(),
            data,
            id: self.event_id.take(),
        };
        (self.options.on_event)(event);
        self.data_lines = Vec::new();
        self.data_bytes = 0;
    }

    /// Upstream `processLine`.
    fn process_line(&mut self, raw_line: &str) -> Result<(), McpClientError> {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.is_empty() {
            self.dispatch();
            return Ok(());
        }
        if line.starts_with(':') {
            return Ok(());
        }
        let (field, raw_value) = match line.find(':') {
            Some(colon) => (&line[..colon], &line[colon + 1..]),
            None => (line, ""),
        };
        let value = raw_value.strip_prefix(' ').unwrap_or(raw_value);
        if field == "data" {
            self.data_bytes += value.len() + usize::from(!self.data_lines.is_empty());
            if self.data_bytes > self.max_event_bytes {
                return Err(McpClientError::Other(format!(
                    "MCP SSE event exceeds {} bytes",
                    self.max_event_bytes
                )));
            }
            self.data_lines.push(value.to_string());
        } else if field == "event" {
            self.event_name = Some(value.to_string());
        } else if field == "id" && !value.contains('\0') {
            self.event_id = Some(value.to_string());
            if let Some(on_id) = &mut self.options.on_id {
                on_id(value);
            }
        } else if field == "retry"
            && !value.is_empty()
            && value.bytes().all(|byte| byte.is_ascii_digit())
        {
            if let Some(on_retry) = &mut self.options.on_retry {
                // `/^\d+$/` then `Number(value)`; u64 saturates where JS
                // would round to an imprecise float.
                on_retry(value.parse::<u64>().unwrap_or(u64::MAX));
            }
        }
        Ok(())
    }

    async fn run(mut self, mut body: ByteStream) -> Result<(), McpClientError> {
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|error| McpClientError::Other(error.to_string()))?;
            self.decode_chunk(&chunk);
            while let Some(newline) = self.buffered.find('\n') {
                let line: String = self.buffered.drain(..newline + 1).collect();
                let line = &line[..line.len() - 1];
                self.process_line(line)?;
            }
            if self.buffered.len() > self.max_event_bytes {
                return Err(McpClientError::Other(format!(
                    "MCP SSE event exceeds {} bytes",
                    self.max_event_bytes
                )));
            }
        }
        // `decoder.decode()` flush + the unterminated final line.
        self.flush_decoder();
        if !self.buffered.is_empty() {
            let tail = std::mem::take(&mut self.buffered);
            self.process_line(&tail)?;
        }
        self.dispatch();
        Ok(())
    }
}

/// Upstream `StreamableHttpReconnectOptions`.
#[derive(Clone, Copy, Debug, Default)]
pub struct StreamableHttpReconnectOptions {
    /// Delay before the first reconnection attempt, unless the server sent a
    /// `retry` field. Default: 1000.
    pub initial_delay_ms: Option<u64>,
    /// Upper bound for the exponential backoff. Default: 30000.
    pub max_delay_ms: Option<u64>,
    /// Consecutive failed attempts before giving up on a stream. Default: 5.
    pub max_retries: Option<u64>,
}

/// Upstream `StreamableHttpTransportOptions`.
#[derive(Clone)]
pub struct StreamableHttpTransportOptions {
    pub url: url::Url,
    pub headers: Vec<(String, String)>,
    pub fetch: Option<McpFetch>,
    /// Open the server-to-client GET stream after initialization. Default:
    /// true.
    pub open_get_stream: Option<bool>,
    pub max_message_bytes: Option<usize>,
    pub auth_provider: Option<Arc<dyn AuthProvider>>,
    pub reconnect: Option<StreamableHttpReconnectOptions>,
}

impl StreamableHttpTransportOptions {
    pub fn new(url: url::Url) -> Self {
        StreamableHttpTransportOptions {
            url,
            headers: Vec::new(),
            fetch: None,
            open_get_stream: None,
            max_message_bytes: None,
            auth_provider: None,
            reconnect: None,
        }
    }
}

struct StreamableHttpShared {
    options: StreamableHttpTransportOptions,
    events: Arc<TransportEvents>,
    fetch: McpFetch,
    cancel: CancellationToken,
    started: AtomicBool,
    closed: AtomicBool,
    session_id: Mutex<Option<String>>,
    protocol_version: Mutex<Option<String>>,
    get_stream_started: AtomicBool,
}

/// Upstream `StreamableHttpTransport`.
pub struct StreamableHttpTransport {
    shared: Arc<StreamableHttpShared>,
}

impl StreamableHttpTransport {
    pub fn new(options: StreamableHttpTransportOptions) -> Self {
        let fetch = options.fetch.clone().unwrap_or_else(default_fetch);
        StreamableHttpTransport {
            shared: Arc::new(StreamableHttpShared {
                events: Arc::new(TransportEvents::default()),
                fetch,
                cancel: CancellationToken::new(),
                started: AtomicBool::new(false),
                closed: AtomicBool::new(false),
                session_id: Mutex::new(None),
                protocol_version: Mutex::new(None),
                get_stream_started: AtomicBool::new(false),
                options,
            }),
        }
    }

    /// Upstream `get sessionId`.
    pub fn session_id(&self) -> Option<String> {
        self.shared
            .session_id
            .lock()
            .expect("session id cannot be poisoned")
            .clone()
    }

    /// Upstream `start()`.
    pub async fn start(&self) -> Result<(), McpClientError> {
        let shared = &self.shared;
        if shared.started.swap(true, Ordering::SeqCst) {
            return Err(McpClientError::Other(
                "MCP Streamable HTTP transport already started".into(),
            ));
        }
        if shared.closed.load(Ordering::SeqCst) {
            return Err(McpClientError::connection_closed());
        }
        Ok(())
    }

    /// Upstream `setProtocolVersion`: drives the `MCP-Protocol-Version`
    /// request header after the handshake.
    pub fn set_protocol_version(&self, version: &str) {
        *self
            .shared
            .protocol_version
            .lock()
            .expect("protocol version cannot be poisoned") = Some(version.to_string());
    }

    /// Upstream `send`. Requests are answered through the JSON body or the
    /// background SSE response stream; notifications and client responses are
    /// fire-and-forget (202/204 expected).
    pub async fn send(&self, message: &JsonRpcMessage) -> Result<(), McpClientError> {
        let shared = &self.shared;
        if !shared.started.load(Ordering::SeqCst) || shared.closed.load(Ordering::SeqCst) {
            return Err(McpClientError::connection_closed());
        }
        let body = message.to_json_string();
        let headers = vec![
            (
                "accept".to_string(),
                "application/json, text/event-stream".to_string(),
            ),
            ("content-type".to_string(), "application/json".to_string()),
        ];
        let response = authorized_fetch(shared, "POST", headers, Some(body)).await?;
        let response = check_response(shared, response).await?;
        capture_session(shared, &response);

        let request_id = match message {
            JsonRpcMessage::Request { id, .. } => Some(id.clone()),
            _ => None,
        };
        let Some(request_id) = request_id else {
            // Notifications and responses are acknowledged with 202 and carry
            // no reply; ignore any body (dropping closes the stream).
            drop(response);
            if let JsonRpcMessage::Notification { method, .. } = message {
                // The server-to-client stream may only open once the session
                // is initialized.
                if method == "notifications/initialized" {
                    start_get_stream(shared);
                }
            }
            return Ok(());
        };

        if response.status == 202 || response.status == 204 {
            let method = match message {
                JsonRpcMessage::Request { method, .. } => method.clone(),
                _ => unreachable!(),
            };
            return Err(McpClientError::Http(McpHttpError::new(
                response.status,
                format!("MCP server accepted request {method} without a response"),
                "",
            )));
        }
        let content_type = content_type_of(&response);
        if content_type.as_deref() == Some("application/json") {
            let text = response.into_text().await.unwrap_or_default();
            let parsed: serde_json::Value = serde_json::from_str(&text)
                .map_err(|error| McpClientError::Other(error.to_string()))?;
            let items: Vec<serde_json::Value> = match parsed {
                Value::Array(items) => items,
                other => vec![other],
            };
            for item in &items {
                let message = parse_json_rpc_message(item)?;
                shared.events.emit_message(&message);
            }
            return Ok(());
        }
        if content_type.as_deref() == Some("text/event-stream") {
            // Consume the response stream in the background, like upstream
            // `void this.consumeResponseStream(...)`.
            let shared_task = Arc::clone(shared);
            tokio::spawn(async move {
                consume_response_stream(&shared_task, response, &request_id).await;
            });
            return Ok(());
        }
        let status = response.status;
        drop(response);
        Err(McpClientError::Http(McpHttpError::new(
            status,
            format!(
                "Unsupported MCP response content type: {}",
                content_type.unwrap_or_else(|| "missing".to_string())
            ),
            "",
        )))
    }

    /// Upstream `close()`.
    pub async fn shutdown(&self) {
        let shared = &self.shared;
        if shared.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        shared.cancel.cancel();
        let session = shared
            .session_id
            .lock()
            .expect("session cannot be poisoned")
            .clone();
        if shared.started.load(Ordering::SeqCst) && session.is_some() {
            // Resolving auth headers failing means the session expires on the
            // server; the DELETE has a 1s timeout and ignores errors.
            if let Ok((headers, _)) = headers(shared, Vec::new()).await {
                let request = FetchRequest {
                    url: shared.options.url.clone(),
                    method: "DELETE".to_string(),
                    headers,
                    body: None,
                    cancel: CancellationToken::new(),
                };
                let fetch = Arc::clone(&shared.fetch);
                let _ = tokio::time::timeout(Duration::from_secs(1), async move {
                    if let Ok(response) = (fetch)(request).await {
                        response.discard();
                    }
                })
                .await;
            }
        }
        shared.events.emit_close();
    }
}

impl McpTransport for StreamableHttpTransport {
    fn start(&self) -> BoxFuture<'_, Result<(), McpClientError>> {
        Box::pin(async move { StreamableHttpTransport::start(self).await })
    }

    fn send(&self, message: &JsonRpcMessage) -> BoxFuture<'_, Result<(), McpClientError>> {
        let shared = Arc::clone(&self.shared);
        let message = message.clone();
        Box::pin(async move {
            StreamableHttpTransport::send(&StreamableHttpTransport { shared }, &message).await
        })
    }

    fn close(&self) -> BoxFuture<'_, Result<(), McpClientError>> {
        Box::pin(async move {
            StreamableHttpTransport::shutdown(self).await;
            Ok(())
        })
    }

    fn on_message(&self, listener: TransportMessageListener) -> Unsubscribe {
        self.shared.events.on_message(listener)
    }

    fn on_error(&self, listener: TransportErrorListener) -> Unsubscribe {
        self.shared.events.on_error(listener)
    }

    fn on_close(&self, listener: TransportCloseListener) -> Unsubscribe {
        self.shared.events.on_close(listener)
    }

    fn set_protocol_version(&self, version: &str) {
        StreamableHttpTransport::set_protocol_version(self, version);
    }
}

fn set_header(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    for (existing_name, existing_value) in headers.iter_mut() {
        if existing_name.eq_ignore_ascii_case(name) {
            *existing_value = value.to_string();
            return;
        }
    }
    headers.push((name.to_string(), value.to_string()));
}

/// Upstream `contentType`: the lowercased content type before the first `;`.
fn content_type_of(response: &FetchResponse) -> Option<String> {
    response.header("content-type").map(|value| {
        value
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase()
    })
}

/// 401, or 403 with an `insufficient_scope` bearer challenge (step-up
/// authorization).
fn needs_authorization(response: &FetchResponse) -> bool {
    if response.status == 401 {
        return true;
    }
    if response.status != 403 {
        return false;
    }
    let challenge = response.header("www-authenticate").unwrap_or("");
    // Upstream `/(?:^|[\s,])error="?insufficient_scope"?/i`.
    let lower = challenge.to_ascii_lowercase();
    for needle in ["error=\"insufficient_scope\"", "error=insufficient_scope"] {
        let mut search_from = 0;
        while let Some(position) = lower[search_from..].find(needle) {
            let absolute = search_from + position;
            let boundary_ok = absolute == 0
                || matches!(
                    lower.as_bytes()[absolute - 1],
                    b' ' | b'\t' | b'\n' | b'\r' | b','
                );
            if boundary_ok {
                return true;
            }
            search_from = absolute + 1;
        }
    }
    false
}

/// Statuses worth retrying when a stream fails to (re)open.
fn is_transient_status(status: u16) -> bool {
    status == 408 || status == 429 || status >= 500
}

/// Network failures and transient statuses are retried; auth, session, and
/// protocol errors are not (upstream `isRetryable`: `McpHttpError` status,
/// `TypeError`, or undici `E*`/`UND_ERR` codes).
fn is_retryable(error: &McpClientError) -> bool {
    match error {
        McpClientError::Http(http) => is_transient_status(http.status),
        // The upstream `TypeError` (network-level fetch failure) analog.
        McpClientError::Network(_) => true,
        _ => false,
    }
}

/// Upstream `describeHttpFailure`.
fn describe_http_failure(status: u16, body: &str) -> String {
    let text = body.trim();
    let snippet = if text.chars().count() > ERROR_MESSAGE_BODY_CHARS {
        let truncated: String = text.chars().take(ERROR_MESSAGE_BODY_CHARS - 3).collect();
        format!("{truncated}...")
    } else {
        text.to_string()
    };
    if snippet.is_empty() {
        format!("MCP HTTP request failed with status {status}")
    } else {
        format!("MCP HTTP request failed with status {status}: {snippet}")
    }
}

#[derive(Default, Clone)]
struct StreamCursor {
    last_event_id: Option<String>,
    retry_ms: Option<u64>,
    /// Whether the stream delivered any event since it was (re)opened.
    received: bool,
}

fn max_delay_of(shared: &StreamableHttpShared) -> u64 {
    shared
        .options
        .reconnect
        .and_then(|options| options.max_delay_ms)
        .unwrap_or(DEFAULT_RECONNECT_MAX_DELAY_MS)
}

fn max_retries_of(shared: &StreamableHttpShared) -> u64 {
    shared
        .options
        .reconnect
        .and_then(|options| options.max_retries)
        .unwrap_or(DEFAULT_RECONNECT_MAX_RETRIES)
}

fn reconnect_delay(
    shared: &StreamableHttpShared,
    attempt: u64,
    server_delay_ms: Option<u64>,
) -> u64 {
    if let Some(server_delay_ms) = server_delay_ms {
        return server_delay_ms;
    }
    let initial = shared
        .options
        .reconnect
        .and_then(|options| options.initial_delay_ms)
        .unwrap_or(DEFAULT_RECONNECT_INITIAL_DELAY_MS);
    let max = max_delay_of(shared);
    let delay = (initial as f64) * 2f64.powi(attempt.min(63) as i32);
    (delay as u64).min(max)
}

/// Resolves false when the transport closed while waiting.
async fn sleep(shared: &StreamableHttpShared, ms: u64) -> bool {
    if shared.cancel.is_cancelled() {
        return false;
    }
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_millis(ms)) => true,
        _ = shared.cancel.cancelled() => false,
    }
}

/// Upstream `headers()`: options headers, then the per-request extras, then
/// session id, protocol version, and bearer token. Returns the token so the
/// 401 context can report which credentials were rejected.
async fn headers(
    shared: &StreamableHttpShared,
    extra: Vec<(String, String)>,
) -> Result<(Vec<(String, String)>, Option<String>), String> {
    let mut merged = shared.options.headers.clone();
    for (name, value) in extra {
        set_header(&mut merged, &name, &value);
    }
    if let Some(session_id) = shared
        .session_id
        .lock()
        .expect("session cannot be poisoned")
        .clone()
    {
        set_header(&mut merged, "Mcp-Session-Id", &session_id);
    }
    if let Some(protocol_version) = shared
        .protocol_version
        .lock()
        .expect("version cannot be poisoned")
        .clone()
    {
        set_header(&mut merged, "MCP-Protocol-Version", &protocol_version);
    }
    let token = match &shared.options.auth_provider {
        Some(provider) => provider.token().await,
        None => None,
    };
    if let Some(token) = &token {
        set_header(&mut merged, "Authorization", &format!("Bearer {token}"));
    }
    Ok((merged, token))
}

/// Fetch with auth headers. A 401 (or a 403 asking for more scope) is handed
/// to the auth provider once, and the request is retried with whatever
/// credentials it left behind.
async fn authorized_fetch(
    shared: &Arc<StreamableHttpShared>,
    method: &str,
    extra: Vec<(String, String)>,
    body: Option<String>,
) -> Result<FetchResponse, McpClientError> {
    let provider = shared.options.auth_provider.clone();
    let mut attempt = 0u32;
    loop {
        let (request_headers, token) = headers(shared, extra.clone())
            .await
            .map_err(McpClientError::Other)?;
        let request = FetchRequest {
            url: shared.options.url.clone(),
            method: method.to_string(),
            headers: request_headers,
            body: body.clone(),
            cancel: shared.cancel.clone(),
        };
        let response = (shared.fetch)(request).await.map_err(map_fetch_error)?;
        let handles_unauthorized = provider
            .as_ref()
            .map(|provider| provider.handles_unauthorized())
            .unwrap_or(false);
        if attempt > 0 || !handles_unauthorized || !needs_authorization(&response) {
            return Ok(response);
        }
        let context = UnauthorizedContext {
            response,
            server_url: shared.options.url.clone(),
            fetch: Arc::clone(&shared.fetch),
            token,
        };
        let provider = provider.as_ref().expect("checked");
        provider
            .on_unauthorized(context)
            .await
            .map_err(|error| McpClientError::Other(error.to_string()))?;
        attempt += 1;
    }
}

fn map_fetch_error(error: crate::mcp::auth_provider::FetchError) -> McpClientError {
    if error.network {
        McpClientError::Network(error.message)
    } else {
        McpClientError::Other(error.message)
    }
}

fn capture_session(shared: &StreamableHttpShared, response: &FetchResponse) {
    if let Some(session_id) = response.header("mcp-session-id") {
        *shared
            .session_id
            .lock()
            .expect("session cannot be poisoned") = Some(session_id.to_string());
    }
}

/// Upstream `checkResponse`: maps non-2xx statuses to the HTTP error classes,
/// reading (and truncating) the error body.
async fn check_response(
    shared: &StreamableHttpShared,
    response: FetchResponse,
) -> Result<FetchResponse, McpClientError> {
    if (200..300).contains(&response.status) {
        return Ok(response);
    }
    let status = response.status;
    let www_authenticate = response.header("www-authenticate").map(str::to_string);
    let body = response
        .into_text()
        .await
        .unwrap_or_default()
        .chars()
        .take(MAX_ERROR_BODY_BYTES)
        .collect::<String>();
    if status == 401 {
        return Err(McpClientError::AuthRequired(McpAuthRequiredError {
            body,
            www_authenticate,
        }));
    }
    let has_session = shared
        .session_id
        .lock()
        .expect("session cannot be poisoned")
        .is_some();
    if status == 404 && has_session {
        return Err(McpClientError::SessionExpired(McpSessionExpiredError {
            body,
        }));
    }
    Err(McpClientError::Http(McpHttpError::new(
        status,
        describe_http_failure(status, &body),
        body,
    )))
}

fn start_get_stream(shared: &Arc<StreamableHttpShared>) {
    if shared.options.open_get_stream == Some(false)
        || shared.get_stream_started.swap(true, Ordering::SeqCst)
        || shared.closed.load(Ordering::SeqCst)
    {
        return;
    }
    let shared_task = Arc::clone(shared);
    tokio::spawn(async move {
        run_get_stream(&shared_task).await;
    });
}

/// Upstream `consumeSse` with the transport cursor and JSON-RPC dispatch.
async fn consume_transport_sse(
    shared: &Arc<StreamableHttpShared>,
    response: FetchResponse,
    cursor: &mut StreamCursor,
    mut on_message: Option<&mut (dyn FnMut(&JsonRpcMessage) + Send)>,
) -> Result<(), McpClientError> {
    // Shared cell so the event/id/retry callbacks can update the cursor.
    struct Updates {
        last_event_id: Option<String>,
        retry_ms: Option<u64>,
        received: bool,
    }
    let updates = Arc::new(Mutex::new(Updates {
        last_event_id: None,
        retry_ms: None,
        received: false,
    }));
    let max_event_bytes = shared
        .options
        .max_message_bytes
        .unwrap_or(DEFAULT_MAX_MESSAGE_BYTES);
    let on_event = {
        let updates = Arc::clone(&updates);
        let shared = Arc::clone(shared);
        move |event: SseEvent| {
            updates.lock().expect("sse updates").received = true;
            // Events without data prime resumption; other event types are not
            // JSON-RPC.
            if event.data.trim().is_empty()
                || event.event.as_ref().is_some_and(|name| name != "message")
            {
                return;
            }
            let parsed = serde_json::from_str::<serde_json::Value>(&event.data)
                .map_err(|error| McpClientError::Other(error.to_string()))
                .and_then(|value| parse_json_rpc_message(&value).map_err(McpClientError::from));
            match parsed {
                Ok(message) => {
                    if let Some(on_message) = on_message.as_deref_mut() {
                        on_message(&message);
                    }
                    shared.events.emit_message(&message);
                }
                Err(error) => shared.events.emit_error(&error),
            }
        }
    };
    let on_id = {
        let updates = Arc::clone(&updates);
        move |id: &str| {
            updates.lock().expect("sse updates").last_event_id = Some(id.to_string());
        }
    };
    let on_retry = {
        let updates = Arc::clone(&updates);
        move |delay_ms: u64| {
            updates.lock().expect("sse updates").retry_ms = Some(delay_ms);
        }
    };
    let result = consume_sse_stream(
        response.body,
        ConsumeSseOptions {
            max_event_bytes: Some(max_event_bytes),
            on_event: Box::new(on_event),
            on_id: Some(Box::new(on_id)),
            on_retry: Some(Box::new(on_retry)),
        },
    )
    .await;
    let mut final_updates = updates.lock().expect("sse updates");
    cursor.last_event_id = final_updates.last_event_id.take();
    cursor.retry_ms = final_updates.retry_ms.take();
    cursor.received = final_updates.received;
    result
}

/// Read the SSE stream answering one request. When the stream ends or breaks
/// before the response arrives and the server assigned event IDs, resume it
/// with GET and `Last-Event-ID`, as the server may close response streams at
/// will. Otherwise only this request fails.
async fn consume_response_stream(
    shared: &Arc<StreamableHttpShared>,
    body: FetchResponse,
    request_id: &JsonRpcId,
) {
    let mut cursor = StreamCursor::default();
    let answered = AtomicBool::new(false);
    let mut on_message = |message: &JsonRpcMessage| {
        if is_json_rpc_response(&message.to_value()) {
            if let JsonRpcMessage::Response(response) = message {
                if response.id() == request_id {
                    answered.store(true, Ordering::SeqCst);
                }
            }
        }
    };
    let mut current: Option<FetchResponse> = Some(body);
    let mut failure: Option<McpClientError> = None;
    let mut attempt: u64 = 0;
    loop {
        if let Some(stream) = current.take() {
            failure = consume_transport_sse(shared, stream, &mut cursor, Some(&mut on_message))
                .await
                .err();
        }
        if answered.load(Ordering::SeqCst) || shared.closed.load(Ordering::SeqCst) {
            return;
        }
        if failure
            .as_ref()
            .is_some_and(|failure| !is_retryable(failure))
        {
            break;
        }
        if cursor.last_event_id.is_none() || attempt >= max_retries_of(shared) {
            break;
        }
        if cursor.received {
            attempt = 0;
        }
        cursor.received = false;
        let delay = reconnect_delay(shared, attempt, cursor.retry_ms);
        attempt += 1;
        if !sleep(shared, delay).await {
            return;
        }
        match open_sse_stream(shared, cursor.last_event_id.clone()).await {
            Ok(stream) => current = stream,
            Err(error) => {
                let retryable = is_retryable(&error);
                failure = Some(error);
                if !retryable {
                    break;
                }
                current = None;
            }
        }
    }
    if shared.closed.load(Ordering::SeqCst) {
        return;
    }
    let reason = match failure {
        None => "stream ended without a response".to_string(),
        Some(failure) => failure.to_string(),
    };
    let message = JsonRpcMessage::Response(JsonRpcResponse::Error {
        id: request_id.clone(),
        error: crate::mcp::protocol::jsonrpc::JsonRpcErrorObject::new(
            JSON_RPC_ERROR_CODES_INTERNAL_ERROR,
            format!("MCP response stream failed: {reason}"),
        ),
    });
    shared.events.emit_message(&message);
}

/// Keep the server-to-client stream open, reconnecting with backoff when it
/// drops (upstream `runGetStream`).
async fn run_get_stream(shared: &Arc<StreamableHttpShared>) {
    let mut cursor = StreamCursor::default();
    let mut attempt: u64 = 0;
    while !shared.closed.load(Ordering::SeqCst) {
        match open_sse_stream(shared, cursor.last_event_id.clone()).await {
            Ok(Some(response)) => {
                let opened_at = Instant::now();
                if let Err(error) = consume_transport_sse(shared, response, &mut cursor, None).await
                {
                    if shared.closed.load(Ordering::SeqCst) {
                        return;
                    }
                    if !is_retryable(&error) {
                        shared.events.emit_error(&error);
                        return;
                    }
                }
                // A stream that stayed up for a while counts as healthy, even
                // if it was idle.
                if cursor.received
                    || opened_at.elapsed() > Duration::from_millis(max_delay_of(shared))
                {
                    attempt = 0;
                }
            }
            Ok(None) => return, // The server does not offer a GET stream.
            Err(error) => {
                if shared.closed.load(Ordering::SeqCst) {
                    return;
                }
                if !is_retryable(&error) {
                    shared.events.emit_error(&error);
                    return;
                }
            }
        }
        cursor.received = false;
        if attempt >= max_retries_of(shared) {
            shared.events.emit_error(&McpClientError::Other(
                "MCP server-to-client stream dropped and could not be reopened".into(),
            ));
            return;
        }
        let delay = reconnect_delay(shared, attempt, cursor.retry_ms);
        attempt += 1;
        if !sleep(shared, delay).await {
            return;
        }
    }
}

/// Open a GET SSE stream. Resolves to `None` when the server answers 405 (no
/// GET stream).
async fn open_sse_stream(
    shared: &Arc<StreamableHttpShared>,
    last_event_id: Option<String>,
) -> Result<Option<FetchResponse>, McpClientError> {
    let mut extra = vec![("accept".to_string(), "text/event-stream".to_string())];
    if let Some(last_event_id) = last_event_id {
        extra.push(("last-event-id".to_string(), last_event_id));
    }
    let response = authorized_fetch(shared, "GET", extra, None).await?;
    if response.status == 405 {
        drop(response);
        return Ok(None);
    }
    let response = check_response(shared, response).await?;
    capture_session(shared, &response);
    let content_type = content_type_of(&response);
    if content_type.as_deref() != Some("text/event-stream") {
        let status = response.status;
        drop(response);
        return Err(McpClientError::Http(McpHttpError::new(
            status,
            format!(
                "Unsupported MCP GET response content type: {}",
                content_type.unwrap_or_else(|| "missing".to_string())
            ),
            "",
        )));
    }
    Ok(Some(response))
}
