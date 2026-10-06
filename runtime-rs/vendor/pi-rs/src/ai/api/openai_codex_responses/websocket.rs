//! The codex websocket transport (upstream
//! `packages/ai/src/api/openai-codex-responses.ts:835-1559`): the connect /
//! acquire / release machinery over session-cached sockets, the debug stats
//! the oracle asserts on, the SSE-fallback session set, the per-attempt
//! `processWebSocketStream` flow, and the websocket-cached continuation
//! delta computation.
//!
//! Port shape: upstream leans on the runtime's event-based `WebSocket`
//! global; the port models a connection as channel pairs ([`WsConnection`])
//! behind a [`WsConnector`] factory so tests can inject mock sockets the way
//! the oracle stubs `globalThis.WebSocket`. The production connector drives
//! a tokio-tungstenite socket through a bridge task. The upgrade request
//! headers are lowercase — upstream passes `headersToRecord(headers)`
//! (`utils/headers.ts` lowercases via `Headers.entries()`), which also makes
//! its `delete wsHeaders["OpenAI-Beta"]` (line 1062) a no-op, so
//! `openai-beta: responses_websockets=2026-02-06` rides the upgrade request
//! exactly as upstream.
//!
//! Deviations, all disclosed in the parent module docs: no bun proxy
//! subclass (lines 971-994 — Rust has no runtime WebSocket global), the
//! session-age clock is injectable for tests only, and socket health is an
//! explicit ready-state byte rather than a runtime `readyState` property.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::mpsc;

use super::{map_codex_event, CodexMapped, CodexStreamError, CODEX_TOOL_CALL_PROVIDERS};
use crate::ai::api::openai_responses_shared::{
    convert_responses_messages, ConvertResponsesMessagesOptions, ResponsesStreamProcessor,
};
use crate::ai::api::REQUEST_WAS_ABORTED;
use crate::ai::now_ms;
use crate::ai::transcript::{normalize_context, Context};
use crate::ai::types::events::AssistantMessageEvent;
use crate::ai::types::message::Message;
use crate::ai::types::Model;
use tokio_util::sync::CancellationToken;

/// Upstream `SESSION_WEBSOCKET_CACHE_TTL_MS` (line 840): how long an idle
/// cached socket stays open.
const SESSION_WEBSOCKET_CACHE_TTL_MS: u64 = 5 * 60 * 1000;
/// Upstream `SESSION_WEBSOCKET_MAX_AGE_MS` (line 841): cached sockets older
/// than this are replaced instead of reused.
const SESSION_WEBSOCKET_MAX_AGE_MS: u64 = 55 * 60 * 1000;
/// Upstream `WEBSOCKET_MESSAGE_TOO_BIG_CLOSE_CODE` (line 62): a close with
/// this code and no reason renders as "message too big".
pub(crate) const WEBSOCKET_MESSAGE_TOO_BIG_CLOSE_CODE: u16 = 1009;

/// Socket health mirror of the runtime `readyState` upstream reads
/// (`isWebSocketReusable`, lines 1020-1024): `1` open/reusable, `3` closed.
const READY_STATE_OPEN: u8 = 1;
const READY_STATE_CLOSED: u8 = 3;

// =============================================================================
// Connection model
// =============================================================================

/// One event surfaced from a websocket, mirroring the upstream `message` /
/// `error` / `close` listener events (`extractWebSocketError` /
/// `extractWebSocketCloseError` shapes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WsEventKind {
    /// A text frame payload.
    Text(String),
    /// A socket-level `error` event carrying its message.
    Error(String),
    /// A `close` event with its code/reason when present.
    Close {
        code: Option<u16>,
        reason: Option<String>,
    },
}

#[derive(Debug)]
pub(crate) struct WsEvent {
    pub kind: WsEventKind,
}

impl WsEvent {
    fn text(payload: impl Into<String>) -> Self {
        WsEvent {
            kind: WsEventKind::Text(payload.into()),
        }
    }

    fn error(message: impl Into<String>) -> Self {
        WsEvent {
            kind: WsEventKind::Error(message.into()),
        }
    }

    fn close(code: Option<u16>, reason: Option<String>) -> Self {
        WsEvent {
            kind: WsEventKind::Close { code, reason },
        }
    }
}

/// Client-to-socket commands (upstream `send`/`close` calls).
#[derive(Debug)]
pub(crate) enum WsOutgoing {
    Send(String),
    Close { code: u16, reason: String },
}

/// A live websocket connection. The production connector backs this with a
/// tokio-tungstenite socket driven by a bridge task; test connectors build
/// it from channels directly.
pub(crate) struct WsConnection {
    pub(crate) outgoing: mpsc::Sender<WsOutgoing>,
    pub(crate) incoming: mpsc::Receiver<WsEvent>,
    pub(crate) ready_state: Arc<AtomicU8>,
}

impl WsConnection {
    /// Upstream `send(data)` (a JSON frame).
    pub(crate) async fn send(&self, payload: String) {
        let _ = self.outgoing.send(WsOutgoing::Send(payload)).await;
    }

    /// Upstream `closeWebSocketSilently` (lines 1030-1034): a best-effort
    /// close that swallows send-backpressure errors.
    pub(crate) fn close_silently(&self, code: u16, reason: &str) {
        let _ = self.outgoing.try_send(WsOutgoing::Close {
            code,
            reason: reason.to_string(),
        });
        self.ready_state.store(READY_STATE_CLOSED, Ordering::SeqCst);
    }

    /// Upstream `isWebSocketReusable` (lines 1020-1024): open, or unknown
    /// (a runtime that does not expose `readyState`) counts as reusable.
    pub(crate) fn is_reusable(&self) -> bool {
        self.ready_state.load(Ordering::SeqCst) == READY_STATE_OPEN
    }

    /// Identity of the underlying socket (upstream compares the cached
    /// entry object with `===`; the port compares the shared ready-state
    /// allocation).
    fn same_socket_as(&self, other: &WsConnection) -> bool {
        Arc::ptr_eq(&self.ready_state, &other.ready_state)
    }
}

impl Drop for WsConnection {
    fn drop(&mut self) {
        // Losing the last handle ends the bridge task (its outgoing sender
        // drops with us), which is the silent close.
        self.ready_state.store(READY_STATE_CLOSED, Ordering::SeqCst);
    }
}

// =============================================================================
// Connector factory (upstream `getWebSocketConstructor` + `connectWebSocket`)
// =============================================================================

/// The transport seam standing in for upstream's runtime `WebSocket`
/// constructor: `(url, headers) -> connection`. Headers arrive lowercase
/// (upstream `headersToRecord`); the returned future is owned so the caller
/// can wrap it in a connect timeout.
pub(crate) trait WsConnector: Send + Sync {
    fn connect(
        &self,
        url: String,
        headers: Vec<(String, String)>,
    ) -> futures::future::BoxFuture<'static, Result<WsConnection, String>>;
}

/// The production connector: tokio-tungstenite `connect_async` with the
/// caller's headers on the upgrade request, then a bridge task shuttling
/// frames between the tungstenite socket and the channel pair.
struct TungsteniteConnector;

impl WsConnector for TungsteniteConnector {
    fn connect(
        &self,
        url: String,
        headers: Vec<(String, String)>,
    ) -> futures::future::BoxFuture<'static, Result<WsConnection, String>> {
        Box::pin(async move {
            use futures::{SinkExt, StreamExt};
            use tokio_tungstenite::tungstenite::client::IntoClientRequest;
            use tokio_tungstenite::tungstenite::protocol::frame::CloseFrame;
            use tokio_tungstenite::tungstenite::Message;

            let mut request = url
                .as_str()
                .into_client_request()
                .map_err(|error| format!("WebSocket error: {error}"))?;
            for (name, value) in headers {
                let header_name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                    .map_err(|error| {
                        format!("Invalid websocket header name \"{name}\": {error}")
                    })?;
                let header_value =
                    reqwest::header::HeaderValue::from_str(&value).map_err(|error| {
                        format!("Invalid websocket header value for \"{name}\": {error}")
                    })?;
                request.headers_mut().insert(header_name, header_value);
            }
            let (socket, _response) = tokio_tungstenite::connect_async(request)
                .await
                .map_err(|error| format!("WebSocket error: {error}"))?;

            let (outgoing_tx, mut outgoing_rx) = mpsc::channel::<WsOutgoing>(16);
            let (incoming_tx, incoming_rx) = mpsc::channel::<WsEvent>(64);
            let ready_state = Arc::new(AtomicU8::new(READY_STATE_OPEN));
            let bridge_ready = ready_state.clone();
            tokio::spawn(async move {
                let (mut sink, mut stream) = socket.split();
                loop {
                    tokio::select! {
                        message = stream.next() => match message {
                            Some(Ok(Message::Text(text))) => {
                                if incoming_tx
                                    .send(WsEvent::text(text.to_string()))
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            // The codex backend sends JSON text frames; binary,
                            // ping/pong and raw frames carry no stream events.
                            Some(Ok(
                                Message::Binary(_)
                                | Message::Ping(_)
                                | Message::Pong(_)
                                | Message::Frame(_),
                            )) => {}
                            Some(Ok(Message::Close(frame))) => {
                                bridge_ready.store(READY_STATE_CLOSED, Ordering::SeqCst);
                                let _ = incoming_tx
                                    .send(WsEvent::close(
                                        frame.as_ref().map(|frame| u16::from(frame.code)),
                                        frame.map(|frame| frame.reason.to_string()),
                                    ))
                                    .await;
                                break;
                            }
                            Some(Err(error)) => {
                                bridge_ready.store(READY_STATE_CLOSED, Ordering::SeqCst);
                                let _ = incoming_tx.send(WsEvent::error(error.to_string())).await;
                                break;
                            }
                            None => {
                                bridge_ready.store(READY_STATE_CLOSED, Ordering::SeqCst);
                                let _ = incoming_tx.send(WsEvent::close(None, None)).await;
                                break;
                            }
                        },
                        command = outgoing_rx.recv() => match command {
                            Some(WsOutgoing::Send(payload)) => {
                                if let Err(error) = sink.send(Message::Text(payload.into())).await {
                                    bridge_ready.store(READY_STATE_CLOSED, Ordering::SeqCst);
                                    let _ = incoming_tx.send(WsEvent::error(error.to_string())).await;
                                    break;
                                }
                            }
                            Some(WsOutgoing::Close { code, reason }) => {
                                bridge_ready.store(READY_STATE_CLOSED, Ordering::SeqCst);
                                let _ = sink
                                    .send(Message::Close(Some(CloseFrame {
                                        code: code.into(),
                                        reason: reason.into(),
                                    })))
                                    .await;
                            }
                            None => break,
                        },
                    }
                }
                bridge_ready.store(READY_STATE_CLOSED, Ordering::SeqCst);
            });
            Ok(WsConnection {
                outgoing: outgoing_tx,
                incoming: incoming_rx,
                ready_state,
            })
        })
    }
}

fn production_connector() -> &'static Arc<dyn WsConnector> {
    static CONNECTOR: OnceLock<Arc<dyn WsConnector>> = OnceLock::new();
    CONNECTOR.get_or_init(|| Arc::new(TungsteniteConnector))
}

static CONNECTOR_OVERRIDE: Mutex<Option<Arc<dyn WsConnector>>> = Mutex::new(None);

fn connector() -> Arc<dyn WsConnector> {
    CONNECTOR_OVERRIDE
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| production_connector().clone())
}

/// Installs a test connector (upstream `vi.stubGlobal("WebSocket", ...)`).
#[cfg(test)]
pub(crate) fn set_connector_for_tests(connector: Option<Arc<dyn WsConnector>>) {
    *CONNECTOR_OVERRIDE.lock().unwrap() = connector;
}

/// Upstream `connectWebSocket` (lines 1049-1125) minus the bun proxy branch:
/// dial through the connector under the connect timeout; a timeout fails
/// with the upstream message text. A cancelled signal aborts the dial with
/// the abort error (upstream rejects `connectWebSocket` on `signal.aborted`).
async fn connect_websocket(
    url: &str,
    headers: &[(String, String)],
    connect_timeout_ms: Option<u64>,
    signal: &CancellationToken,
) -> Result<WsConnection, String> {
    if signal.is_cancelled() {
        return Err(REQUEST_WAS_ABORTED.to_string());
    }
    let future = connector().connect(url.to_string(), headers.to_vec());
    let timeout = connect_timeout_ms.filter(|ms| *ms > 0);
    match timeout {
        Some(ms) => {
            let dial = tokio::select! {
                biased;
                _ = signal.cancelled() => return Err(REQUEST_WAS_ABORTED.to_string()),
                result = tokio::time::timeout(Duration::from_millis(ms), future) => result,
            };
            match dial {
                Ok(result) => result,
                Err(_elapsed) => Err(format!("WebSocket connect timeout after {ms}ms")),
            }
        }
        None => {
            tokio::select! {
                biased;
                _ = signal.cancelled() => Err(REQUEST_WAS_ABORTED.to_string()),
                result = future => result,
            }
        }
    }
}

// =============================================================================
// Session state (upstream lines 884-960)
// =============================================================================

/// Upstream `CachedWebSocketContinuationState` (lines 853-857).
#[derive(Debug, Clone)]
pub(crate) struct ContinuationState {
    pub last_request_body: Value,
    pub last_response_id: String,
    pub last_response_items: Vec<Value>,
}

/// Upstream `CachedWebSocketConnection` (lines 859-865). The socket is
/// `Option` because the busy socket is held outside the map while a request
/// streams (the receiver is not cloneable, unlike the JS object handle).
pub(crate) struct CachedConnection {
    socket: Option<WsConnection>,
    busy: bool,
    created_at: u64,
    idle_expiry: Option<tokio::task::JoinHandle<()>>,
    pub(crate) continuation: Option<ContinuationState>,
}

/// Upstream `OpenAICodexWebSocketDebugStats` (lines 867-882).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct WsDebugStats {
    pub requests: u64,
    pub connections_created: u64,
    pub connections_reused: u64,
    pub cached_context_requests: u64,
    pub store_true_requests: u64,
    pub full_context_requests: u64,
    pub delta_requests: u64,
    pub last_input_items: usize,
    pub last_delta_input_items: Option<usize>,
    pub last_previous_response_id: Option<String>,
    pub websocket_failures: u64,
    pub sse_fallbacks: u64,
    pub websocket_fallback_active: Option<bool>,
    pub last_websocket_error: Option<String>,
}

static SESSION_CACHE: std::sync::LazyLock<
    Mutex<HashMap<String, HashMap<String, CachedConnection>>>,
> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
static DEBUG_STATS: std::sync::LazyLock<Mutex<HashMap<String, WsDebugStats>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
static SSE_FALLBACK_SESSIONS: std::sync::LazyLock<Mutex<HashSet<String>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashSet::new()));

/// Session wall clock, overridable for the connection-age tests (upstream
/// uses `vi.setSystemTime`).
fn ws_now_ms() -> u64 {
    match CLOCK_OVERRIDE_MS.load(Ordering::SeqCst) {
        0 => now_ms().max(0) as u64,
        ms => ms,
    }
}

static CLOCK_OVERRIDE_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(test)]
pub(crate) fn set_clock_for_tests(ms: u64) {
    CLOCK_OVERRIDE_MS.store(ms, Ordering::SeqCst);
}

/// Mutates one session's stats entry, creating it first (upstream
/// `getOrCreateWebSocketDebugStats` plus in-place mutation).
fn stats_mut<R>(session_id: &str, apply: impl FnOnce(&mut WsDebugStats) -> R) -> R {
    let mut stats = DEBUG_STATS.lock().unwrap();
    apply(stats.entry(session_id.to_string()).or_default())
}

/// Upstream `getOpenAICodexWebSocketDebugStats` (lines 908-911).
#[cfg(test)]
pub(crate) fn websocket_debug_stats(session_id: &str) -> Option<WsDebugStats> {
    DEBUG_STATS.lock().unwrap().get(session_id).cloned()
}

/// Upstream `resetOpenAICodexWebSocketDebugStats` (lines 913-921) plus the
/// fallback-set half of the oracle's `afterEach` cleanup.
#[cfg(test)]
pub(crate) fn reset_websocket_state(session_id: Option<&str>) {
    match session_id {
        Some(session_id) => {
            DEBUG_STATS.lock().unwrap().remove(session_id);
            SSE_FALLBACK_SESSIONS.lock().unwrap().remove(session_id);
        }
        None => {
            DEBUG_STATS.lock().unwrap().clear();
            SSE_FALLBACK_SESSIONS.lock().unwrap().clear();
        }
    }
}

/// Upstream `closeOpenAICodexWebSocketSessions` (lines 923-937): close every
/// cached socket and drop the cache (idle timers abort with their handles).
#[cfg(test)]
pub(crate) fn close_websocket_sessions(session_id: Option<&str>) {
    let mut cache = SESSION_CACHE.lock().unwrap();
    let drained: Vec<HashMap<String, CachedConnection>> = match session_id {
        Some(session_id) => cache
            .remove(session_id)
            .map(|entries| vec![entries])
            .unwrap_or_default(),
        None => cache.drain().map(|(_, entries)| entries).collect(),
    };
    for account_entries in drained {
        for (_, mut entry) in account_entries {
            close_cached_entry(&mut entry);
        }
    }
}

#[cfg(test)]
fn close_cached_entry(entry: &mut CachedConnection) {
    if let Some(handle) = entry.idle_expiry.take() {
        handle.abort();
    }
    if let Some(socket) = entry.socket.take() {
        socket.close_silently(1000, "debug_close");
    }
}

/// Serializes the tests that touch the process-global websocket state.
#[cfg(test)]
pub(crate) async fn lock_global_state_for_tests() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    LOCK.lock().await
}

/// Upstream `isWebSocketSseFallbackActive` (lines 941-943).
pub(crate) fn is_websocket_sse_fallback_active(session_id: Option<&str>) -> bool {
    match session_id {
        Some(session_id) => SSE_FALLBACK_SESSIONS.lock().unwrap().contains(session_id),
        None => false,
    }
}

/// Upstream `recordWebSocketSseFallback` (lines 945-950).
pub(crate) fn record_websocket_sse_fallback(session_id: Option<&str>) {
    let Some(session_id) = session_id else {
        return;
    };
    let active = is_websocket_sse_fallback_active(Some(session_id));
    stats_mut(session_id, |stats| {
        stats.sse_fallbacks += 1;
        stats.websocket_fallback_active = Some(active);
    });
}

/// Upstream `recordWebSocketFailure` (lines 952-960).
pub(crate) fn record_websocket_failure(session_id: Option<&str>, error: &CodexStreamError) {
    let Some(session_id) = session_id else {
        return;
    };
    SSE_FALLBACK_SESSIONS
        .lock()
        .unwrap()
        .insert(session_id.to_string());
    let message = error.message.clone();
    stats_mut(session_id, |stats| {
        stats.websocket_failures += 1;
        stats.last_websocket_error = Some(message);
        stats.websocket_fallback_active = Some(true);
    });
}

/// Upstream `isWebSocketSessionExpired` (lines 1026-1028).
fn is_websocket_session_expired(entry: &CachedConnection) -> bool {
    ws_now_ms().saturating_sub(entry.created_at) >= SESSION_WEBSOCKET_MAX_AGE_MS
}

/// Upstream `scheduleSessionWebSocketExpiry` (lines 1036-1047).
fn schedule_session_websocket_expiry(session_id: &str, account_id: &str) {
    let handle = {
        let mut cache = SESSION_CACHE.lock().unwrap();
        let Some(entry) = cache
            .get_mut(session_id)
            .and_then(|accounts| accounts.get_mut(account_id))
        else {
            return;
        };
        if let Some(previous) = entry.idle_expiry.take() {
            previous.abort();
        }
        let session = session_id.to_string();
        let account = account_id.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(SESSION_WEBSOCKET_CACHE_TTL_MS)).await;
            let mut cache = SESSION_CACHE.lock().unwrap();
            let Some(accounts) = cache.get_mut(&session) else {
                return;
            };
            let close_now = accounts
                .get(&account)
                .map(|entry| !entry.busy)
                .unwrap_or(false);
            if close_now {
                if let Some(mut entry) = accounts.remove(&account) {
                    if let Some(socket) = entry.socket.take() {
                        socket.close_silently(1000, "idle_timeout");
                    }
                }
                if accounts.is_empty() {
                    cache.remove(&session);
                }
            }
        })
    };
    let mut cache = SESSION_CACHE.lock().unwrap();
    if let Some(entry) = cache
        .get_mut(session_id)
        .and_then(|accounts| accounts.get_mut(account_id))
    {
        entry.idle_expiry = Some(handle);
    }
}

/// Evicts one account's cached socket, closing it with the given close
/// reason (upstream lines 1158-1160 / 1190-1194).
fn evict_cached_connection(
    cache: &mut HashMap<String, HashMap<String, CachedConnection>>,
    session_id: &str,
    account_id: &str,
    reason: &str,
) {
    let Some(accounts) = cache.get_mut(session_id) else {
        return;
    };
    if let Some(mut entry) = accounts.remove(account_id) {
        if let Some(handle) = entry.idle_expiry.take() {
            handle.abort();
        }
        if let Some(socket) = entry.socket.take() {
            socket.close_silently(1000, reason);
        }
    }
    if accounts.is_empty() {
        cache.remove(session_id);
    }
}

// =============================================================================
// Acquire / release (upstream lines 1127-1222)
// =============================================================================

/// The result of [`acquire_websocket`]: the live socket plus whether it is
/// bound to a cache slot (`cached`) and whether that slot was reused.
pub(crate) struct AcquiredWebSocket {
    pub socket: WsConnection,
    pub cached: bool,
    pub reused: bool,
}

enum AcquirePlan {
    /// Take the existing idle reusable socket (busy flag already set).
    Reuse,
    /// The cached slot is busy: dial a fresh one-shot socket that always
    /// closes on release (upstream lines 1180-1189).
    FreshOneShot,
    /// No usable cached slot: dial and cache.
    FreshCached,
}

/// Upstream `acquireWebSocket` (lines 1127-1222). The request signal aborts
/// any dial (`connectWebSocket`'s signal parameter).
pub(crate) async fn acquire_websocket(
    url: &str,
    headers: &[(String, String)],
    session_id: Option<&str>,
    account_id: &str,
    connect_timeout_ms: Option<u64>,
    signal: &CancellationToken,
) -> Result<AcquiredWebSocket, String> {
    let Some(session_id) = session_id else {
        let socket = connect_websocket(url, headers, connect_timeout_ms, signal).await?;
        return Ok(AcquiredWebSocket {
            socket,
            cached: false,
            reused: false,
        });
    };

    // Upstream clears the idle timer as soon as a cached entry is looked at
    // (lines 1153-1156), then classifies: busy -> one-shot; expired ->
    // evict("connection_age_limit") + fresh; reusable -> reuse; otherwise
    // evict("done") + fresh.
    let plan = {
        let mut cache = SESSION_CACHE.lock().unwrap();
        match cache.get_mut(session_id) {
            None => AcquirePlan::FreshCached,
            Some(accounts) => match accounts.get_mut(account_id) {
                None => AcquirePlan::FreshCached,
                Some(entry) => {
                    if let Some(previous) = entry.idle_expiry.take() {
                        previous.abort();
                    }
                    if entry.busy {
                        AcquirePlan::FreshOneShot
                    } else if is_websocket_session_expired(entry) {
                        evict_cached_connection(
                            &mut cache,
                            session_id,
                            account_id,
                            "connection_age_limit",
                        );
                        AcquirePlan::FreshCached
                    } else if entry
                        .socket
                        .as_ref()
                        .map(WsConnection::is_reusable)
                        .unwrap_or(true)
                    {
                        entry.busy = true;
                        AcquirePlan::Reuse
                    } else {
                        evict_cached_connection(&mut cache, session_id, account_id, "done");
                        AcquirePlan::FreshCached
                    }
                }
            },
        }
    };

    match plan {
        AcquirePlan::Reuse => {
            let socket = {
                let mut cache = SESSION_CACHE.lock().unwrap();
                cache
                    .get_mut(session_id)
                    .and_then(|accounts| accounts.get_mut(account_id))
                    .and_then(|entry| entry.socket.take())
                    .expect("reuse plan marked the slot busy")
            };
            Ok(AcquiredWebSocket {
                socket,
                cached: true,
                reused: true,
            })
        }
        AcquirePlan::FreshOneShot => {
            let socket = connect_websocket(url, headers, connect_timeout_ms, signal).await?;
            Ok(AcquiredWebSocket {
                socket,
                cached: false,
                reused: false,
            })
        }
        AcquirePlan::FreshCached => {
            let socket = connect_websocket(url, headers, connect_timeout_ms, signal).await?;
            let mut cache = SESSION_CACHE.lock().unwrap();
            let accounts = cache.entry(session_id.to_string()).or_default();
            let entry = accounts
                .entry(account_id.to_string())
                .or_insert(CachedConnection {
                    socket: None,
                    busy: true,
                    created_at: ws_now_ms(),
                    idle_expiry: None,
                    continuation: None,
                });
            entry.busy = true;
            entry.socket = Some(socket);
            let socket = entry.socket.take().expect("just stored");
            Ok(AcquiredWebSocket {
                socket,
                cached: true,
                reused: false,
            })
        }
    }
}

/// Upstream `release({ keep })` (lines 1167-1177, 1209-1220): return a
/// healthy cached socket to its slot (re-arming the idle timer), or close
/// and evict it. One-shot sockets always close.
pub(crate) fn release_websocket(
    socket: WsConnection,
    session_id: Option<&str>,
    account_id: &str,
    cached: bool,
    keep: bool,
) {
    let Some(session_id) = session_id.filter(|_| cached) else {
        socket.close_silently(1000, "done");
        return;
    };

    if !keep || !socket.is_reusable() {
        socket.close_silently(1000, "done");
        let mut cache = SESSION_CACHE.lock().unwrap();
        // Only evict when the slot still belongs to this socket (upstream
        // checks `currentEntries?.get(accountId) === cached`). A busy slot
        // holds `socket: None` while we own the socket, which is ours.
        let ours = match cache
            .get(session_id)
            .and_then(|accounts| accounts.get(account_id))
        {
            Some(entry) => match entry.socket.as_ref() {
                Some(entry_socket) => entry_socket.same_socket_as(&socket),
                None => entry.busy,
            },
            None => false,
        };
        if ours {
            evict_cached_connection(&mut cache, session_id, account_id, "done");
        }
        return;
    }

    let mut cache = SESSION_CACHE.lock().unwrap();
    let slot_free = cache
        .get(session_id)
        .and_then(|accounts| accounts.get(account_id))
        .map(|entry| {
            entry.busy
                && entry
                    .socket
                    .as_ref()
                    .map(|entry_socket| entry_socket.same_socket_as(&socket))
                    .unwrap_or(true)
        })
        .unwrap_or(false);
    if slot_free {
        if let Some(entry) = cache
            .get_mut(session_id)
            .and_then(|accounts| accounts.get_mut(account_id))
        {
            entry.socket = Some(socket);
            entry.busy = false;
        }
        drop(cache);
        schedule_session_websocket_expiry(session_id, account_id);
    } else {
        // The slot moved on (evicted or replaced); dropping closes quietly.
        drop(socket);
    }
}

// =============================================================================
// Continuation delta (upstream lines 1399-1451)
// =============================================================================

/// Upstream `requestBodyWithoutInput` (lines 1399-1402).
fn request_body_without_input(body: &Value) -> Value {
    match body {
        Value::Object(map) => {
            let mut map = map.clone();
            map.shift_remove("input");
            map.shift_remove("previous_response_id");
            Value::Object(map)
        }
        other => other.clone(),
    }
}

/// Upstream `responseInputsEqual` (lines 1404-1406): JSON-string equality.
/// Arbitrary nested request/response JSON retains insertion order via
/// `serde_json`'s `preserve_order` feature. Do not replace this with `Value`
/// equality or sorted JSON: upstream treats reordered object fields as a
/// different continuation prefix (including tool arguments and metadata).
fn response_inputs_equal(a: &Value, b: &Value) -> bool {
    let empty = Value::Array(Vec::new());
    let a = if a.is_null() { &empty } else { a };
    let b = if b.is_null() { &empty } else { b };
    serde_json::to_string(a).unwrap_or_default() == serde_json::to_string(b).unwrap_or_default()
}

/// Upstream `requestBodiesMatchExceptInput` (lines 1408-1410).
fn request_bodies_match_except_input(a: &Value, b: &Value) -> bool {
    serde_json::to_string(&request_body_without_input(a)).unwrap_or_default()
        == serde_json::to_string(&request_body_without_input(b)).unwrap_or_default()
}

/// Upstream `getCachedWebSocketInputDelta` (lines 1412-1432): the input
/// suffix beyond the cached baseline (last request input + the response
/// items it produced), or `None` when the request is not a pure append.
fn get_cached_websocket_input_delta(
    body: &Value,
    continuation: &ContinuationState,
) -> Option<Value> {
    if !request_bodies_match_except_input(body, &continuation.last_request_body) {
        return None;
    }
    let empty: Vec<Value> = Vec::new();
    let current_input = body
        .get("input")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let last_input = continuation
        .last_request_body
        .get("input")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let mut baseline: Vec<&Value> = last_input.iter().collect();
    baseline.extend(continuation.last_response_items.iter());
    if current_input.len() < baseline.len() {
        return None;
    }
    let prefix_matches = current_input
        .iter()
        .zip(baseline.iter())
        .all(|(current, base)| response_inputs_equal(current, base));
    if !prefix_matches {
        return None;
    }
    Some(Value::Array(current_input[baseline.len()..].to_vec()))
}

/// Upstream `buildCachedWebSocketRequestBody` (lines 1434-1451): reduce the
/// request to `previous_response_id` + the input delta, or clear the
/// continuation slot and send the full body.
pub(crate) fn build_cached_websocket_request_body(
    continuation_slot: &mut Option<ContinuationState>,
    body: &Value,
) -> Value {
    let Some(continuation) = continuation_slot.as_mut() else {
        return body.clone();
    };
    let delta = get_cached_websocket_input_delta(body, continuation);
    match delta.filter(|_| !continuation.last_response_id.is_empty()) {
        Some(delta) => {
            let mut request = body.clone();
            if let Value::Object(map) = &mut request {
                map.insert(
                    "previous_response_id".into(),
                    Value::String(continuation.last_response_id.clone()),
                );
                map.insert("input".into(), delta);
            }
            request
        }
        None => {
            *continuation_slot = None;
            body.clone()
        }
    }
}

// =============================================================================
// Per-attempt stream (upstream processWebSocketStream + parseWebSocket,
// lines 1281-1397 and 1467-1559)
// =============================================================================

/// Everything one websocket attempt needs, owned by the caller's request
/// flow (upstream passes the same values positionally).
pub(crate) struct WsAttempt<'a> {
    pub url: String,
    /// The full request body (the `response.create` payload minus the
    /// envelope type).
    pub body: &'a Value,
    pub ws_headers: &'a [(String, String)],
    pub model: &'a Model,
    pub processor: &'a mut ResponsesStreamProcessor,
    pub tx: &'a mpsc::Sender<AssistantMessageEvent>,
    /// Shared with the whole request: once any attempt emitted `start`, the
    /// SSE path must not emit it again.
    pub start_emitted: &'a mut bool,
    /// Set when THIS attempt emitted its first event (upstream
    /// `websocketStarted`): failures after it never fall back to SSE.
    pub attempt_started: &'a mut bool,
    /// Upstream `idleTimeoutMs` — `options.timeoutMs` doubles as the
    /// websocket inter-message idle timeout.
    pub idle_timeout_ms: Option<u64>,
    pub connect_timeout_ms: Option<u64>,
    pub cache_session_id: Option<&'a str>,
    pub account_id: &'a str,
    /// Upstream `useCachedContext`: transport `websocket-cached` or `auto`.
    pub use_cached_context: bool,
    /// The request's grammar map (upstream line 1541 forwards it to the
    /// continuation conversion).
    pub grammar_tool_input_properties: &'a std::collections::HashMap<String, String>,
    /// The request signal (upstream `options?.signal`): aborts the dial and
    /// the message reads.
    pub signal: CancellationToken,
}

/// One attempt over the websocket transport. Returns the `end_turn` value
/// captured from the terminal event, when the terminal event carried one.
pub(crate) async fn process_websocket_stream(
    attempt: WsAttempt<'_>,
) -> Result<Option<bool>, CodexStreamError> {
    let WsAttempt {
        url,
        body,
        ws_headers,
        model,
        processor,
        tx,
        start_emitted,
        attempt_started,
        idle_timeout_ms,
        connect_timeout_ms,
        cache_session_id,
        account_id,
        use_cached_context,
        grammar_tool_input_properties,
        signal,
    } = attempt;

    let acquired = acquire_websocket(
        &url,
        ws_headers,
        cache_session_id,
        account_id,
        connect_timeout_ms,
        &signal,
    )
    .await
    .map_err(CodexStreamError::plain)?;
    let reused = acquired.reused;
    let cached = acquired.cached;
    let mut socket = acquired.socket;

    // Take the slot's continuation for the duration of the request; success
    // stores a fresh one (cached context only), failure clears it.
    let mut continuation: Option<ContinuationState> = if cached {
        let cache = SESSION_CACHE.lock().unwrap();
        cache
            .get(cache_session_id.unwrap_or_default())
            .and_then(|accounts| accounts.get(account_id))
            .and_then(|entry| entry.continuation.clone())
    } else {
        None
    };

    let request_body = if use_cached_context && cached {
        build_cached_websocket_request_body(&mut continuation, body)
    } else {
        body.clone()
    };

    if let Some(session) = cache_session_id {
        let input_items = request_body
            .get("input")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        let previous_response_id = request_body
            .get("previous_response_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let store_true = request_body.get("store") == Some(&Value::Bool(true));
        stats_mut(session, |stats| {
            stats.requests += 1;
            if reused {
                stats.connections_reused += 1;
            } else {
                stats.connections_created += 1;
            }
            if use_cached_context {
                stats.cached_context_requests += 1;
            }
            if store_true {
                stats.store_true_requests += 1;
            }
            stats.last_input_items = input_items;
            match previous_response_id {
                Some(previous_response_id) => {
                    stats.delta_requests += 1;
                    stats.last_delta_input_items = Some(input_items);
                    stats.last_previous_response_id = Some(previous_response_id);
                }
                None => {
                    stats.full_context_requests += 1;
                    stats.last_delta_input_items = None;
                    stats.last_previous_response_id = None;
                }
            }
        });
    }

    // `socket.send(JSON.stringify({ type: "response.create", ...body }))`.
    let mut envelope = json!({"type": "response.create"});
    if let (Value::Object(envelope), Value::Object(body_map)) = (&mut envelope, &request_body) {
        for (key, value) in body_map {
            envelope.insert(key.clone(), value.clone());
        }
    }
    socket.send(envelope.to_string()).await;

    let outcome = drive_websocket_stream(DriveWebSocketStream {
        socket: &mut socket,
        processor,
        tx,
        start_emitted,
        attempt_started,
        idle_timeout_ms,
        signal: &signal,
    })
    .await;

    let end_turn = match &outcome {
        Ok(end_turn) => *end_turn,
        Err(_) => None,
    };

    // Continuation bookkeeping (upstream lines 1532-1554): success with
    // cached context and a response id stores the response items; failure
    // clears the slot.
    let mut stored_continuation: Option<ContinuationState> = continuation;
    match &outcome {
        Err(_) => stored_continuation = None,
        Ok(_) => {
            if use_cached_context && cached {
                if let Some(response_id) = processor.output().response_id.clone() {
                    let context = normalize_context(&Context {
                        system_prompt: None,
                        messages: vec![Message::Assistant(processor.output().clone())],
                        tools: None,
                    });
                    let allowed: std::collections::HashSet<String> = CODEX_TOOL_CALL_PROVIDERS
                        .iter()
                        .map(|provider| (*provider).to_string())
                        .collect();
                    let items = convert_responses_messages(
                        model,
                        &context,
                        &allowed,
                        &ConvertResponsesMessagesOptions {
                            include_system_prompt: Some(false),
                            grammar_tool_input_properties: grammar_tool_input_properties.clone(),
                            ..Default::default()
                        },
                    )
                    .unwrap_or_default();
                    let response_items: Vec<Value> = items
                        .into_iter()
                        .filter(|item| {
                            !matches!(
                                item.get("type").and_then(Value::as_str),
                                Some("function_call_output") | Some("custom_tool_call_output")
                            )
                        })
                        .collect();
                    stored_continuation = Some(ContinuationState {
                        last_request_body: body.clone(),
                        last_response_id: response_id,
                        last_response_items: response_items,
                    });
                }
            }
        }
    }
    if cached {
        if let Some(entry) = SESSION_CACHE
            .lock()
            .unwrap()
            .get_mut(cache_session_id.unwrap_or_default())
            .and_then(|accounts| accounts.get_mut(account_id))
        {
            entry.continuation = stored_continuation;
        }
    }

    release_websocket(
        socket,
        cache_session_id,
        account_id,
        cached,
        outcome.is_ok(),
    );

    outcome.map(|_| end_turn)
}

/// The parse/process loop (upstream `parseWebSocket` + `mapCodexEvents` +
/// `startWebSocketOutputOnFirstEvent`). Returns the terminal event's
/// `end_turn` value on success.
async fn drive_websocket_stream(
    drive: DriveWebSocketStream<'_>,
) -> Result<Option<bool>, CodexStreamError> {
    let DriveWebSocketStream {
        socket,
        processor,
        tx,
        start_emitted,
        attempt_started,
        idle_timeout_ms,
        signal,
    } = drive;

    let incoming = &mut socket.incoming;
    let mut saw_completion = false;
    let mut failed: Option<CodexStreamError> = None;
    let mut end_turn: Option<bool> = None;
    let mut first_event_seen = false;

    loop {
        // Upstream `parseWebSocket` aborts the read when the signal fires
        // ("Request was aborted"); the select breaks the receive the moment
        // the token cancels, ahead of the idle timeout.
        enum Received {
            Event(Option<WsEvent>),
            IdleTimeout,
        }
        let received = tokio::select! {
            biased;
            _ = signal.cancelled() => {
                failed = Some(CodexStreamError::plain(REQUEST_WAS_ABORTED.to_string()));
                break;
            }
            received = async {
                match idle_timeout_ms {
                    Some(ms) if ms > 0 => {
                        match tokio::time::timeout(Duration::from_millis(ms), incoming.recv())
                            .await
                        {
                            Ok(event) => Received::Event(event),
                            Err(_elapsed) => Received::IdleTimeout,
                        }
                    }
                    _ => Received::Event(incoming.recv().await),
                }
            } => received,
        };
        let event = match received {
            Received::IdleTimeout => {
                socket.close_silently(1000, "idle_timeout");
                let ms = idle_timeout_ms.unwrap_or(0);
                failed = Some(CodexStreamError::plain(format!(
                    "WebSocket idle timeout after {ms}ms"
                )));
                break;
            }
            Received::Event(event) => event,
        };
        let Some(event) = event else {
            // The bridge ended without a close event.
            if !saw_completion {
                failed = Some(CodexStreamError::plain("WebSocket closed"));
            }
            break;
        };

        match event.kind {
            WsEventKind::Text(text) => {
                let parsed: Value = match serde_json::from_str(&text) {
                    Ok(parsed) => parsed,
                    Err(error) => {
                        failed = Some(CodexStreamError::protocol(format!(
                            "Invalid Codex WebSocket JSON: {error}"
                        )));
                        break;
                    }
                };
                // Upstream order: mapCodexEvents throws for error/failed
                // events BEFORE they reach startWebSocketOutputOnFirstEvent,
                // so only yieldable events trigger the start emission.
                let mapped = map_codex_event(&parsed, &mut end_turn);
                if !matches!(mapped, CodexMapped::Skip | CodexMapped::Error(_)) && !first_event_seen
                {
                    first_event_seen = true;
                    *attempt_started = true;
                    if !*start_emitted {
                        *start_emitted = true;
                        let _ = tx
                            .send(AssistantMessageEvent::Start {
                                message: processor.output().clone(),
                            })
                            .await;
                    }
                }
                if matches!(
                    parsed.get("type").and_then(Value::as_str),
                    Some("response.completed")
                        | Some("response.done")
                        | Some("response.incomplete")
                ) {
                    saw_completion = true;
                }
                match mapped {
                    CodexMapped::Skip => {}
                    CodexMapped::Event(event) => {
                        if let Err(error) = processor.process_event(&event, tx).await {
                            failed = Some(CodexStreamError::plain(error));
                            break;
                        }
                    }
                    CodexMapped::Error(error) => {
                        failed = Some(error);
                        break;
                    }
                    CodexMapped::Terminal(event) => {
                        if let Err(error) = processor.process_event(&event, tx).await {
                            failed = Some(CodexStreamError::plain(error));
                            break;
                        }
                        break;
                    }
                }
            }
            WsEventKind::Error(message) => {
                failed = Some(CodexStreamError::plain(if message.is_empty() {
                    "WebSocket error".to_string()
                } else {
                    message
                }));
                break;
            }
            WsEventKind::Close { code, reason } => {
                if !saw_completion {
                    failed = Some(websocket_close_error(code, reason));
                }
                break;
            }
        }
    }

    match failed {
        Some(error) => Err(error),
        None if !saw_completion => Err(CodexStreamError::plain(
            "WebSocket stream closed before response.completed",
        )),
        None => Ok(end_turn),
    }
}

struct DriveWebSocketStream<'a> {
    socket: &'a mut WsConnection,
    processor: &'a mut ResponsesStreamProcessor,
    tx: &'a mpsc::Sender<AssistantMessageEvent>,
    start_emitted: &'a mut bool,
    attempt_started: &'a mut bool,
    idle_timeout_ms: Option<u64>,
    /// The request signal (upstream `parseWebSocket`'s `signal`): breaks the
    /// message read on abort.
    signal: &'a CancellationToken,
}

/// Upstream `extractWebSocketCloseError` (lines 1245-1262).
fn websocket_close_error(code: Option<u16>, reason: Option<String>) -> CodexStreamError {
    let code_text = code.map(|code| format!(" {code}")).unwrap_or_default();
    let has_reason = reason
        .as_ref()
        .map(|reason| !reason.is_empty())
        .unwrap_or(false);
    let reason_text = if has_reason {
        format!(" {}", reason.clone().unwrap_or_default())
    } else if code == Some(WEBSOCKET_MESSAGE_TOO_BIG_CLOSE_CODE) {
        " message too big".to_string()
    } else {
        String::new()
    };
    CodexStreamError::close(
        code,
        reason,
        format!("WebSocket closed{code_text}{reason_text}")
            .trim()
            .to_string(),
    )
}

#[cfg(test)]
mod json_order_tests {
    use super::*;

    #[test]
    fn json_order_request_rest_keeps_survivors_and_does_not_mutate_input() {
        for raw in [
            r#"{"input":[],"model":"m","temperature":0.2,"z":{"q":1,"a":2},"previous_response_id":"x"}"#,
            r#"{"model":"m","previous_response_id":"y","temperature":0.2,"input":[1],"z":{"q":1,"a":2}}"#,
            r#"{"model":"m","temperature":0.2,"z":{"q":1,"a":2},"input":[]}"#,
        ] {
            let value: Value = serde_json::from_str(raw).unwrap();
            assert_eq!(
                request_body_without_input(&value).to_string(),
                r#"{"model":"m","temperature":0.2,"z":{"q":1,"a":2}}"#
            );
            assert_eq!(value.to_string(), raw);
        }
    }

    #[test]
    fn json_order_websocket_continuation_uses_ordered_bytes_not_value_equality() {
        let last: Value = serde_json::from_str(
            r#"{"input":[{"role":"user","content":"A"}],"model":"m","temperature":0.2,"z":1}"#,
        )
        .unwrap();
        let current: Value = serde_json::from_str(r#"{"model":"m","input":[{"role":"user","content":"A"},{"role":"assistant","content":"ok"},{"role":"user","content":"B"}],"temperature":0.2,"z":1,"previous_response_id":"x"}"#).unwrap();
        let continuation = ContinuationState {
            last_request_body: last,
            last_response_id: "r".into(),
            last_response_items: vec![json!({"role":"assistant","content":"ok"})],
        };
        assert_eq!(
            get_cached_websocket_input_delta(&current, &continuation)
                .unwrap()
                .to_string(),
            r#"[{"role":"user","content":"B"}]"#
        );
        let mut changed = current.clone();
        changed["input"][0] = json!({"content":"A","role":"user"});
        assert_eq!(
            changed["input"], current["input"],
            "Value equality is intentionally order-insensitive"
        );
        assert!(get_cached_websocket_input_delta(&changed, &continuation).is_none());
        let reordered = json!({"temperature":0.2,"model":"m","z":1,"input":current["input"]});
        assert!(get_cached_websocket_input_delta(&reordered, &continuation).is_none());
        assert!(!response_inputs_equal(
            &json!([{"z":1,"a":2}]),
            &json!([{"a":2,"z":1}])
        ));
        assert!(!request_bodies_match_except_input(
            &json!({"model":"m","z":{"q":1,"a":2}}),
            &json!({"model":"m","z":{"a":2,"q":1}})
        ));
        assert!(response_inputs_equal(&Value::Null, &json!([])));
    }
}
