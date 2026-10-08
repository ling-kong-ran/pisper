//! The MCP client, ported from upstream `packages/mcp/src/client.ts`.
//!
//! One [`McpClient`] drives one [`McpTransport`]: the initialize handshake,
//! request timeouts/cancellation/progress, server-to-client requests
//! (`ping`, `roots/list`, custom handlers), notification listeners, and
//! result validation with the upstream error strings.
//!
//! Port notes (disclosed divergences):
//! - Upstream rejects requests with one of several Error classes; the port
//!   folds them into [`McpClientError`] (Display strings are identical).
//! - Upstream `AbortSignal.reason` carries the abort cause;
//!   `CancellationToken` has none, so cancellation notifications use
//!   `McpRequestOptions::abort_reason` when set, else `"Aborted"`.
//! - `timeoutMs` of `0` disables the request timer exactly like upstream's
//!   `timeoutMs <= 0` check; `u64::MAX` stands in for `Infinity`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::Value;
use tokio::sync::oneshot;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::mcp::protocol::content::CallToolResult;
use crate::mcp::protocol::jsonrpc::{
    JsonRpcId, JsonRpcMessage, JsonRpcResponse, McpClientError, McpError,
    JSON_RPC_ERROR_CODES_INTERNAL_ERROR, JSON_RPC_ERROR_CODES_INVALID_REQUEST,
    JSON_RPC_ERROR_CODES_METHOD_NOT_FOUND,
};
use crate::mcp::protocol::types::{
    InitializeResult, ListResourceTemplatesResult, ListResourcesResult, ReadResourceResult,
    Resource, ResourceTemplate, Root, Tool, LATEST_PROTOCOL_VERSION, SUPPORTED_PROTOCOL_VERSIONS,
};
use crate::mcp::transports::{McpTransport, Unsubscribe};

const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 30_000;
const MAX_LIST_PAGES: usize = 1_000;

/// Upstream `RequestHandler` result context: the port hands server-request
/// handlers a cancellation token standing in for the upstream `AbortSignal`.
#[derive(Clone)]
pub struct RequestContext {
    pub signal: CancellationToken,
}

pub type RequestHandlerFuture = BoxFuture<'static, Result<Value, McpClientError>>;
pub type RequestHandler =
    std::sync::Arc<dyn Fn(Option<Value>, RequestContext) -> RequestHandlerFuture + Send + Sync>;
pub type NotificationListener = std::sync::Arc<dyn Fn(&Value) + Send + Sync>;
pub type ErrorListener = std::sync::Arc<dyn Fn(&McpClientError) + Send + Sync>;
pub type CloseListener = std::sync::Arc<dyn Fn() + Send + Sync>;
pub type ProgressListener = std::sync::Arc<dyn Fn(&Value) + Send + Sync>;

/// Where `roots` come from: a fixed list or an async provider (upstream
/// `readonly Root[] | (() => readonly Root[] | Promise<readonly Root[]>)`).
#[derive(Clone)]
pub enum RootsProvider {
    List(std::sync::Arc<Vec<Root>>),
    Callback(std::sync::Arc<dyn Fn() -> BoxFuture<'static, Vec<Root>> + Send + Sync>),
}

/// Upstream `McpClientOptions extends Implementation`.
#[derive(Clone)]
pub struct McpClientOptions {
    pub name: String,
    pub version: String,
    pub title: Option<String>,
    /// Raw `ClientCapabilities` object (insertion order is preserved into the
    /// initialize params).
    pub capabilities: Option<serde_json::Map<String, Value>>,
    pub protocol_version: Option<String>,
    pub request_timeout_ms: Option<u64>,
    /// When set, the client advertises the `roots` capability and serves
    /// `roots/list`.
    pub roots: Option<RootsProvider>,
}

impl McpClientOptions {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        McpClientOptions {
            name: name.into(),
            version: version.into(),
            title: None,
            capabilities: None,
            protocol_version: None,
            request_timeout_ms: None,
            roots: None,
        }
    }
}

/// Upstream `McpRequestOptions`. `timeout_ms: Some(0)` disables the timer.
/// `abort_reason` is the `AbortSignal.reason` stand-in: `CancellationToken`
/// carries no reason, so the `notifications/cancelled` reason uses this when
/// set (upstream `String(signal.reason ?? "Aborted")`).
#[derive(Clone, Default)]
pub struct McpRequestOptions {
    pub signal: Option<CancellationToken>,
    pub abort_reason: Option<String>,
    pub timeout_ms: Option<u64>,
    pub on_progress: Option<ProgressListener>,
}

/// Upstream `ClientState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientState {
    Idle,
    Connecting,
    Connected,
    Closed,
}

impl ClientState {
    fn as_str(&self) -> &'static str {
        match self {
            ClientState::Idle => "idle",
            ClientState::Connecting => "connecting",
            ClientState::Connected => "connected",
            ClientState::Closed => "closed",
        }
    }
}

/// A resettable deadline shared between the pending entry and its timer task
/// (upstream clears/re-arms a `setTimeout`).
struct DeadlineCell {
    state: Mutex<Option<Instant>>,
    notify: tokio::sync::Notify,
}

impl DeadlineCell {
    fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(DeadlineCell {
            state: Mutex::new(None),
            notify: tokio::sync::Notify::new(),
        })
    }

    fn set(&self, deadline: Option<Instant>) {
        *self.state.lock().expect("deadline cannot be poisoned") = deadline;
        self.notify.notify_waiters();
    }

    fn get(&self) -> Option<Instant> {
        *self.state.lock().expect("deadline cannot be poisoned")
    }
}

struct PendingRequest {
    responder: oneshot::Sender<Result<Value, McpClientError>>,
    deadline: Option<std::sync::Arc<DeadlineCell>>,
    timeout_ms: u64,
    signal: Option<CancellationToken>,
    cancellable: bool,
    on_progress: Option<ProgressListener>,
    progress_token: Option<JsonRpcId>,
    timeout_task: Option<tokio::task::AbortHandle>,
    abort_task: Option<tokio::task::AbortHandle>,
}

/// Interior state behind `Arc`; one mutex mirrors upstream's synchronous
/// mutation of the client fields.
struct Inner {
    options: McpClientOptions,
    state: Mutex<ClientState>,
    transport: Mutex<Option<std::sync::Arc<dyn McpTransport>>>,
    next_request_id: AtomicI64,
    server_info: Mutex<Option<Value>>,
    server_capabilities: Mutex<Option<serde_json::Map<String, Value>>>,
    instructions: Mutex<Option<String>>,
    protocol_version: Mutex<Option<String>>,
    pending: Mutex<HashMap<JsonRpcId, PendingRequest>>,
    progress_requests: Mutex<HashMap<JsonRpcId, JsonRpcId>>,
    incoming: Mutex<HashMap<JsonRpcId, CancellationToken>>,
    request_handlers: Mutex<Vec<(String, RequestHandler)>>,
    notification_listeners: Mutex<Vec<(String, u64, NotificationListener)>>,
    error_listeners: Mutex<Vec<(u64, ErrorListener)>>,
    close_listeners: Mutex<Vec<(u64, CloseListener)>>,
    next_listener_id: AtomicU64,
    transport_listener_disposers: Mutex<Vec<Unsubscribe>>,
}

/// The MCP client. All methods are callable through `&self` from any task
/// after `connect`; see the module docs for the upstream mapping. Cloning
/// shares one connection (the handle is an Arc, like passing the upstream
/// object around).
#[derive(Clone)]
pub struct McpClient {
    inner: std::sync::Arc<Inner>,
}

impl McpClient {
    /// Upstream `new McpClient(options)`: registers the default `ping`
    /// handler and, when roots are configured, a `roots/list` handler.
    pub fn new(options: McpClientOptions) -> Self {
        let mut handlers: Vec<(String, RequestHandler)> = Vec::new();
        handlers.push((
            "ping".to_string(),
            std::sync::Arc::new(
                |_params: Option<Value>, _context: RequestContext| -> RequestHandlerFuture {
                    Box::pin(async { Ok(serde_json::json!({})) })
                },
            ) as RequestHandler,
        ));
        if let Some(roots) = &options.roots {
            let roots = roots.clone();
            handlers.push((
                "roots/list".to_string(),
                std::sync::Arc::new(
                    move |_params: Option<Value>,
                          _context: RequestContext|
                          -> RequestHandlerFuture {
                        let roots = roots.clone();
                        Box::pin(async move {
                            let list = match &roots {
                                RootsProvider::List(list) => list.as_ref().clone(),
                                RootsProvider::Callback(callback) => callback().await,
                            };
                            let roots_array: Vec<Value> = list
                                .iter()
                                .map(|root| {
                                    serde_json::to_value(root)
                                        .expect("Root serialization cannot fail")
                                })
                                .collect();
                            Ok(serde_json::json!({ "roots": roots_array }))
                        })
                    },
                ) as RequestHandler,
            ));
        }
        McpClient {
            inner: std::sync::Arc::new(Inner {
                options,
                state: Mutex::new(ClientState::Idle),
                transport: Mutex::new(None),
                next_request_id: AtomicI64::new(1),
                server_info: Mutex::new(None),
                server_capabilities: Mutex::new(None),
                instructions: Mutex::new(None),
                protocol_version: Mutex::new(None),
                pending: Mutex::new(HashMap::new()),
                progress_requests: Mutex::new(HashMap::new()),
                incoming: Mutex::new(HashMap::new()),
                request_handlers: Mutex::new(handlers),
                notification_listeners: Mutex::new(Vec::new()),
                error_listeners: Mutex::new(Vec::new()),
                close_listeners: Mutex::new(Vec::new()),
                next_listener_id: AtomicU64::new(1),
                transport_listener_disposers: Mutex::new(Vec::new()),
            }),
        }
    }

    pub fn connection_state(&self) -> ClientState {
        *self.inner.state.lock().expect("state cannot be poisoned")
    }

    pub fn server_info(&self) -> Option<Value> {
        self.inner
            .server_info
            .lock()
            .expect("server info cannot be poisoned")
            .clone()
    }

    pub fn server_capabilities(&self) -> Option<serde_json::Map<String, Value>> {
        self.inner
            .server_capabilities
            .lock()
            .expect("capabilities cannot be poisoned")
            .clone()
    }

    pub fn instructions(&self) -> Option<String> {
        self.inner
            .instructions
            .lock()
            .expect("instructions cannot be poisoned")
            .clone()
    }

    pub fn protocol_version(&self) -> Option<String> {
        self.inner
            .protocol_version
            .lock()
            .expect("protocol version cannot be poisoned")
            .clone()
    }

    /// Upstream `connect(transport)`: performs the initialize handshake and
    /// sends `notifications/initialized`. On failure the client is closed
    /// before the error propagates.
    pub async fn connect(
        &self,
        transport: std::sync::Arc<dyn McpTransport>,
    ) -> Result<InitializeResult, McpClientError> {
        {
            let mut state = self.inner.state.lock().expect("state cannot be poisoned");
            if *state != ClientState::Idle {
                return Err(McpClientError::Other(format!(
                    "Cannot connect MCP client in {} state",
                    state.as_str()
                )));
            }
            *state = ClientState::Connecting;
        }
        *self
            .inner
            .transport
            .lock()
            .expect("transport cannot be poisoned") = Some(Arc::clone(&transport));

        let events = TransportPump {
            client: std::sync::Arc::clone(&self.inner),
        };
        let disposers = vec![
            transport.on_message(std::sync::Arc::new(move |message| {
                events.handle_message(message)
            })),
            transport.on_error(std::sync::Arc::new({
                let client = std::sync::Arc::clone(&self.inner);
                move |error| Self::emit_error(&client, error)
            })),
            transport.on_close(std::sync::Arc::new({
                let client = std::sync::Arc::clone(&self.inner);
                move || Self::handle_transport_close(&client)
            })),
        ];
        *self
            .inner
            .transport_listener_disposers
            .lock()
            .expect("disposers cannot be poisoned") = disposers;

        let result = async {
            transport.start().await?;
            // `{ ...this.options.capabilities }`, then `roots: {}` appended
            // when roots are configured and the caller did not set them.
            let mut capabilities = self.inner.options.capabilities.clone().unwrap_or_default();
            if self.inner.options.roots.is_some() && !capabilities.contains_key("roots") {
                capabilities.insert("roots".into(), serde_json::json!({}));
            }
            let mut client_info = serde_json::Map::new();
            client_info.insert("name".into(), Value::from(self.inner.options.name.clone()));
            client_info.insert(
                "version".into(),
                Value::from(self.inner.options.version.clone()),
            );
            if let Some(title) = &self.inner.options.title {
                client_info.insert("title".into(), Value::from(title.clone()));
            }
            let mut initialize_params = serde_json::Map::new();
            initialize_params.insert(
                "protocolVersion".into(),
                Value::from(
                    self.inner
                        .options
                        .protocol_version
                        .clone()
                        .unwrap_or_else(|| LATEST_PROTOCOL_VERSION.to_string()),
                ),
            );
            initialize_params.insert("capabilities".into(), Value::Object(capabilities));
            initialize_params.insert("clientInfo".into(), Value::Object(client_info));

            let raw = self
                .request_internal(
                    "initialize",
                    Some(initialize_params),
                    &McpRequestOptions::default(),
                    true,
                )
                .await?;
            let result = validate_initialize_result(&raw)?;
            if !(SUPPORTED_PROTOCOL_VERSIONS.contains(&result.protocol_version.as_str())) {
                return Err(McpClientError::Other(format!(
                    "MCP server selected unsupported protocol version {}",
                    result.protocol_version
                )));
            }
            *self
                .inner
                .protocol_version
                .lock()
                .expect("version cannot be poisoned") = Some(result.protocol_version.clone());
            *self
                .inner
                .server_info
                .lock()
                .expect("info cannot be poisoned") = Some(
                serde_json::to_value(&result.server_info)
                    .expect("Implementation serialization cannot fail"),
            );
            *self
                .inner
                .server_capabilities
                .lock()
                .expect("capabilities cannot be poisoned") = Some(result.capabilities.clone());
            *self
                .inner
                .instructions
                .lock()
                .expect("instructions cannot be poisoned") = result.instructions.clone();
            transport.set_protocol_version(&result.protocol_version);
            self.notify_internal("notifications/initialized", None, true)
                .await?;
            *self.inner.state.lock().expect("state cannot be poisoned") = ClientState::Connected;
            Ok(result)
        }
        .await;

        match result {
            Ok(initialized) => Ok(initialized),
            Err(error) => {
                let _ = self.close().await;
                Err(error)
            }
        }
    }

    /// Upstream `request(method, params?, options?)`.
    pub async fn request(
        &self,
        method: &str,
        params: Option<serde_json::Map<String, Value>>,
        options: McpRequestOptions,
    ) -> Result<Value, McpClientError> {
        self.request_internal(method, params, &options, false).await
    }

    /// Upstream `notify(method, params?)`.
    pub async fn notify(
        &self,
        method: &str,
        params: Option<serde_json::Map<String, Value>>,
    ) -> Result<(), McpClientError> {
        self.notify_internal(method, params, false).await
    }

    /// Upstream `setRequestHandler`. Returns the unsubscribe function.
    pub fn set_request_handler(&self, method: &str, handler: RequestHandler) -> Unsubscribe {
        let inner = &self.inner;
        let mut handlers = inner
            .request_handlers
            .lock()
            .expect("handlers cannot be poisoned");
        // Replace any existing handler for the method (upstream Map.set).
        if let Some(slot) = handlers.iter_mut().find(|(existing, _)| existing == method) {
            slot.1 = handler;
        } else {
            handlers.push((method.to_string(), handler));
        }
        let method = method.to_string();
        let inner = std::sync::Arc::clone(inner);
        Unsubscribe::new(move || {
            let mut guard = inner
                .request_handlers
                .lock()
                .expect("handlers cannot be poisoned");
            // Upstream removes only if the registered handler is still ours;
            // the port cannot compare function objects, so an unsubscribe
            // removes the current handler for the method.
            if let Some(position) = guard.iter().position(|(existing, _)| *existing == method) {
                guard.remove(position);
            }
        })
    }

    /// Upstream `onNotification`.
    pub fn on_notification(&self, method: &str, listener: NotificationListener) -> Unsubscribe {
        let id = self.inner.next_listener_id.fetch_add(1, Ordering::Relaxed);
        self.inner
            .notification_listeners
            .lock()
            .expect("listeners cannot be poisoned")
            .push((method.to_string(), id, listener));
        let inner = std::sync::Arc::clone(&self.inner);
        Unsubscribe::new(move || {
            inner
                .notification_listeners
                .lock()
                .expect("listeners cannot be poisoned")
                .retain(|(_, listener_id, _)| *listener_id != id);
        })
    }

    /// Upstream `onError`.
    pub fn on_error(&self, listener: ErrorListener) -> Unsubscribe {
        let id = self.inner.next_listener_id.fetch_add(1, Ordering::Relaxed);
        self.inner
            .error_listeners
            .lock()
            .expect("listeners cannot be poisoned")
            .push((id, listener));
        let inner = std::sync::Arc::clone(&self.inner);
        Unsubscribe::new(move || {
            inner
                .error_listeners
                .lock()
                .expect("listeners cannot be poisoned")
                .retain(|(listener_id, _)| *listener_id != id);
        })
    }

    /// Called once when the connection closes, whether the transport dropped
    /// or `close()` was called.
    pub fn on_close(&self, listener: CloseListener) -> Unsubscribe {
        let id = self.inner.next_listener_id.fetch_add(1, Ordering::Relaxed);
        self.inner
            .close_listeners
            .lock()
            .expect("listeners cannot be poisoned")
            .push((id, listener));
        let inner = std::sync::Arc::clone(&self.inner);
        Unsubscribe::new(move || {
            inner
                .close_listeners
                .lock()
                .expect("listeners cannot be poisoned")
                .retain(|(listener_id, _)| *listener_id != id);
        })
    }

    /// Upstream `ping(options?)`.
    pub async fn ping(&self, options: McpRequestOptions) -> Result<(), McpClientError> {
        self.request("ping", None, options).await.map(|_| ())
    }

    /// Upstream `listTools(options?)`: every page of `tools/list`.
    pub async fn list_tools(
        &self,
        options: McpRequestOptions,
    ) -> Result<Vec<Tool>, McpClientError> {
        let items = self
            .list_all("tools/list", "tools", is_tool_value, options)
            .await?;
        Ok(items
            .into_iter()
            .map(|item| serde_json::from_value(Value::Object(item)).expect("validated tool shape"))
            .collect())
    }

    /// Upstream `listResources(options?)`: every page of `resources/list`.
    pub async fn list_resources(
        &self,
        options: McpRequestOptions,
    ) -> Result<Vec<Resource>, McpClientError> {
        let items = self
            .list_all("resources/list", "resources", is_resource_value, options)
            .await?;
        items
            .iter()
            .map(|item| {
                serde_json::from_value(Value::Object(to_resource(item))).map_err(into_invalid)
            })
            .collect()
    }

    /// Upstream `listResourcesPage(cursor?, options?)`: one page.
    pub async fn list_resources_page(
        &self,
        cursor: Option<String>,
        options: McpRequestOptions,
    ) -> Result<ListResourcesResult, McpClientError> {
        let page = self
            .list_page(
                "resources/list",
                "resources",
                is_resource_value,
                cursor,
                &options,
            )
            .await?;
        let items: Result<Vec<Resource>, McpClientError> = page
            .items
            .iter()
            .map(|item| {
                serde_json::from_value(Value::Object(to_resource(item))).map_err(into_invalid)
            })
            .collect();
        Ok(ListResourcesResult {
            resources: items?,
            next_cursor: page.next_cursor,
            meta: page.meta,
        })
    }

    /// Upstream `listResourceTemplates(options?)`: every page.
    pub async fn list_resource_templates(
        &self,
        options: McpRequestOptions,
    ) -> Result<Vec<ResourceTemplate>, McpClientError> {
        let items = self
            .list_all(
                "resources/templates/list",
                "resourceTemplates",
                is_resource_template_value,
                options,
            )
            .await?;
        items
            .iter()
            .map(|item| {
                serde_json::from_value(Value::Object(to_resource_template(item)))
                    .map_err(into_invalid)
            })
            .collect()
    }

    /// Upstream `listResourceTemplatesPage(cursor?, options?)`: one page.
    pub async fn list_resource_templates_page(
        &self,
        cursor: Option<String>,
        options: McpRequestOptions,
    ) -> Result<ListResourceTemplatesResult, McpClientError> {
        let page = self
            .list_page(
                "resources/templates/list",
                "resourceTemplates",
                is_resource_template_value,
                cursor,
                &options,
            )
            .await?;
        let templates: Result<Vec<ResourceTemplate>, McpClientError> = page
            .items
            .iter()
            .map(|item| {
                serde_json::from_value(Value::Object(to_resource_template(item)))
                    .map_err(into_invalid)
            })
            .collect();
        Ok(ListResourceTemplatesResult {
            resource_templates: templates?,
            next_cursor: page.next_cursor,
            meta: page.meta,
        })
    }

    /// Upstream `readResource(uri, options?)`.
    pub async fn read_resource(
        &self,
        uri: &str,
        options: McpRequestOptions,
    ) -> Result<ReadResourceResult, McpClientError> {
        let mut params = serde_json::Map::new();
        params.insert("uri".into(), Value::from(uri));
        let result = self
            .request("resources/read", Some(params), options)
            .await?;
        validate_read_resource_result(&result)
    }

    /// Upstream `callTool(name, args?, options?)`.
    pub async fn call_tool(
        &self,
        name: &str,
        args: Option<serde_json::Map<String, Value>>,
        options: McpRequestOptions,
    ) -> Result<CallToolResult, McpClientError> {
        let mut params = serde_json::Map::new();
        params.insert("name".into(), Value::from(name));
        if let Some(args) = args {
            params.insert("arguments".into(), Value::Object(args));
        }
        let result = self.request("tools/call", Some(params), options).await?;
        validate_call_tool_result(&result)
    }

    /// Upstream `close()`.
    pub async fn close(&self) -> Result<(), McpClientError> {
        let transport = self
            .inner
            .transport
            .lock()
            .expect("transport cannot be poisoned")
            .take();
        self.dispose_transport_listeners();
        Self::mark_closed(&self.inner, &McpClientError::connection_closed());
        if let Some(transport) = transport {
            transport.close().await?;
        }
        Ok(())
    }

    // -- internals ----------------------------------------------------------

    fn dispose_transport_listeners(&self) {
        for disposer in self
            .inner
            .transport_listener_disposers
            .lock()
            .expect("disposers cannot be poisoned")
            .drain(..)
        {
            disposer.unsubscribe();
        }
    }

    fn emit_error(inner: &std::sync::Arc<Inner>, error: &McpClientError) {
        let listeners = inner
            .error_listeners
            .lock()
            .expect("listeners cannot be poisoned")
            .clone();
        for (_, listener) in listeners {
            listener(error);
        }
    }

    fn handle_transport_close(inner: &std::sync::Arc<Inner>) {
        Self::mark_closed(inner, &McpClientError::connection_closed());
    }

    /// Idempotent: rejects in-flight requests, aborts server requests we are
    /// serving, and flips the state.
    fn mark_closed(inner: &std::sync::Arc<Inner>, error: &McpClientError) {
        let was_closed;
        {
            let mut state = inner.state.lock().expect("state cannot be poisoned");
            was_closed = *state == ClientState::Closed;
            *state = ClientState::Closed;
        }
        Self::reject_pending(inner, error);
        let controllers: Vec<CancellationToken> = inner
            .incoming
            .lock()
            .expect("incoming cannot be poisoned")
            .drain()
            .map(|(_, controller)| controller)
            .collect();
        for controller in controllers {
            controller.cancel();
        }
        if was_closed {
            return;
        }
        let listeners = inner
            .close_listeners
            .lock()
            .expect("listeners cannot be poisoned")
            .clone();
        for (_, listener) in listeners {
            listener();
        }
    }

    fn reject_pending(inner: &std::sync::Arc<Inner>, error: &McpClientError) {
        let mut pending = inner.pending.lock().expect("pending cannot be poisoned");
        for (_, mut entry) in std::mem::take(&mut *pending) {
            Self::detach_entry(&mut entry);
            entry.responder.send(Err(error.clone())).ok();
        }
    }

    fn detach_entry(entry: &mut PendingRequest) {
        if let Some(task) = entry.timeout_task.take() {
            task.abort();
        }
        if let Some(task) = entry.abort_task.take() {
            task.abort();
        }
    }

    fn remove_pending(inner: &std::sync::Arc<Inner>, id: &JsonRpcId) -> Option<PendingRequest> {
        let mut pending = inner.pending.lock().expect("pending cannot be poisoned");
        let mut entry = pending.remove(id)?;
        if let Some(token) = &entry.progress_token {
            inner
                .progress_requests
                .lock()
                .expect("progress map cannot be poisoned")
                .remove(token);
        }
        Self::detach_entry(&mut entry);
        Some(entry)
    }

    /// Upstream `cancelPending`.
    fn cancel_pending(
        inner: &std::sync::Arc<Inner>,
        id: &JsonRpcId,
        error: McpClientError,
        notify_server: bool,
        reason: Option<String>,
    ) {
        let Some(entry) = Self::remove_pending(inner, id) else {
            return;
        };
        entry.responder.send(Err(error)).ok();
        if notify_server {
            let transport = inner
                .transport
                .lock()
                .expect("transport cannot be poisoned")
                .clone();
            if let Some(transport) = transport {
                let mut params = serde_json::Map::new();
                params.insert(
                    "requestId".into(),
                    match id {
                        JsonRpcId::String(text) => Value::from(text.clone()),
                        JsonRpcId::Number(number) => Value::Number(number.clone()),
                    },
                );
                if let Some(reason) = reason {
                    if !reason.is_empty() {
                        params.insert("reason".into(), Value::from(reason));
                    }
                }
                let notification = JsonRpcMessage::Notification {
                    method: "notifications/cancelled".to_string(),
                    params: Some(Value::Object(params)),
                };
                let inner = std::sync::Arc::clone(inner);
                tokio::spawn(async move {
                    if let Err(send_error) = transport.send(&notification).await {
                        Self::emit_error(&inner, &send_error);
                    }
                });
            }
        }
    }

    /// Upstream `requireTransport`.
    fn require_transport(
        inner: &std::sync::Arc<Inner>,
        allow_connecting: bool,
    ) -> Result<std::sync::Arc<dyn McpTransport>, McpClientError> {
        let state = *inner.state.lock().expect("state cannot be poisoned");
        let transport = inner
            .transport
            .lock()
            .expect("transport cannot be poisoned")
            .clone();
        if let (Some(transport), true) = (
            transport,
            state == ClientState::Connected
                || (allow_connecting && state == ClientState::Connecting),
        ) {
            return Ok(transport);
        }
        Err(McpClientError::connection_closed_with(format!(
            "MCP client is {}",
            state.as_str()
        )))
    }

    async fn request_internal(
        &self,
        method: &str,
        params: Option<serde_json::Map<String, Value>>,
        options: &McpRequestOptions,
        allow_connecting: bool,
    ) -> Result<Value, McpClientError> {
        let inner = std::sync::Arc::clone(&self.inner);
        let transport = Self::require_transport(&inner, allow_connecting)?;
        if options
            .signal
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(McpClientError::aborted());
        }
        let id_number = inner.next_request_id.fetch_add(1, Ordering::SeqCst);
        let id = JsonRpcId::Number(serde_json::Number::from(id_number));
        let progress_token = if options.on_progress.is_some() {
            Some(id.clone())
        } else {
            None
        };
        let params_value = params.map(Value::Object);
        let request_params = match &progress_token {
            None => params_value,
            Some(token) => {
                let token_value = match token {
                    JsonRpcId::String(text) => Value::from(text.clone()),
                    JsonRpcId::Number(number) => Value::Number(number.clone()),
                };
                // Upstream `{ ...params, _meta: { ...(isObject(params?._meta)
                // ? params._meta : {}), progressToken } }`: the spread keeps
                // every original key in place, and the `_meta` insert
                // replaces an existing key in position or appends.
                let mut request_params = match &params_value {
                    Some(Value::Object(object)) => object.clone(),
                    _ => serde_json::Map::new(),
                };
                let mut meta = serde_json::Map::new();
                if let Some(Value::Object(existing)) = request_params.get("_meta") {
                    meta = existing.clone();
                }
                meta.insert("progressToken".into(), token_value);
                request_params.insert("_meta".into(), Value::Object(meta));
                Some(Value::Object(request_params))
            }
        };
        let message = JsonRpcMessage::Request {
            id: id.clone(),
            method: method.to_string(),
            params: request_params,
        };

        let (responder, receiver) = oneshot::channel();
        let timeout_ms = options
            .timeout_ms
            .or(inner.options.request_timeout_ms)
            .unwrap_or(DEFAULT_REQUEST_TIMEOUT_MS);
        let entry_deadline = if timeout_ms == 0 {
            // Upstream: `!Number.isFinite(entry.timeoutMs) || timeoutMs <= 0`
            // disables the timer entirely.
            None
        } else {
            Some(DeadlineCell::new())
        };
        let mut pending_entry = PendingRequest {
            responder,
            deadline: entry_deadline,
            timeout_ms,
            signal: options.signal.clone(),
            cancellable: method != "initialize",
            on_progress: options.on_progress.clone(),
            progress_token: progress_token.clone(),
            timeout_task: None,
            abort_task: None,
        };

        // The timeout watcher (upstream `armTimeout`).
        if let Some(deadline_cell) = &pending_entry.deadline {
            let deadline = Instant::now() + Duration::from_millis(timeout_ms);
            deadline_cell.set(Some(deadline));
            let cell = std::sync::Arc::clone(deadline_cell);
            let inner_task = std::sync::Arc::clone(&inner);
            let id_task = id.clone();
            let cancellable = pending_entry.cancellable;
            pending_entry.timeout_task = Some(
                tokio::spawn(async move {
                    loop {
                        let Some(current) = cell.get() else {
                            return;
                        };
                        tokio::select! {
                            _ = tokio::time::sleep_until(current) => {
                                // Re-arm supersedes the fired timer.
                                if cell.get() == Some(current) {
                                    Self::cancel_pending(
                                        &inner_task,
                                        &id_task,
                                        McpClientError::timeout(timeout_ms),
                                        cancellable,
                                        Some("Request timed out".to_string()),
                                    );
                                    return;
                                }
                            }
                            _ = cell.notify.notified() => {}
                        }
                    }
                })
                .abort_handle(),
            );
        }

        // The abort watcher (upstream the `abort` event listener).
        if let Some(signal) = &pending_entry.signal {
            let signal = signal.clone();
            let inner_task = std::sync::Arc::clone(&inner);
            let id_task = id.clone();
            let cancellable = pending_entry.cancellable;
            // Upstream reason: `String(signal.reason ?? "Aborted")`; the port
            // carries the caller-supplied `abort_reason` (a
            // `CancellationToken` has no reason of its own).
            let reason = options
                .abort_reason
                .clone()
                .unwrap_or_else(|| "Aborted".to_string());
            pending_entry.abort_task = Some(
                tokio::spawn(async move {
                    signal.cancelled().await;
                    Self::cancel_pending(
                        &inner_task,
                        &id_task,
                        McpClientError::aborted(),
                        cancellable,
                        Some(reason),
                    );
                })
                .abort_handle(),
            );
        }

        inner
            .pending
            .lock()
            .expect("pending cannot be poisoned")
            .insert(id.clone(), pending_entry);
        if let Some(token) = &progress_token {
            inner
                .progress_requests
                .lock()
                .expect("progress map cannot be poisoned")
                .insert(token.clone(), id.clone());
        }

        if let Err(send_error) = transport.send(&message).await {
            // Upstream `transport.send(message).catch((error) =>
            // this.cancelPending(id, error, false))`.
            Self::cancel_pending(&inner, &id, send_error, false, None);
        }

        match receiver.await {
            Ok(result) => result,
            // The responder was dropped without sending: entry removed by a
            // concurrent close that already rejected through `reject_pending`.
            Err(_) => Err(McpClientError::connection_closed()),
        }
    }

    async fn notify_internal(
        &self,
        method: &str,
        params: Option<serde_json::Map<String, Value>>,
        allow_connecting: bool,
    ) -> Result<(), McpClientError> {
        let transport = Self::require_transport(&self.inner, allow_connecting)?;
        transport
            .send(&JsonRpcMessage::Notification {
                method: method.to_string(),
                params: params.map(Value::Object),
            })
            .await
    }

    fn handle_response(inner: &std::sync::Arc<Inner>, response: JsonRpcResponse) {
        let id = response.id().clone();
        let Some(entry) = Self::remove_pending(inner, &id) else {
            Self::emit_error(
                inner,
                &McpClientError::Other(format!(
                    "Received response for unknown MCP request {}",
                    id.to_display_string()
                )),
            );
            return;
        };
        match response {
            JsonRpcResponse::Success { result, .. } => {
                entry.responder.send(Ok(result)).ok();
            }
            JsonRpcResponse::Error { error, .. } => {
                let mut mcp_error = McpError::new(error.code, error.message);
                if let Some(data) = error.data {
                    mcp_error = mcp_error.with_data(data);
                }
                entry
                    .responder
                    .send(Err(McpClientError::Mcp(mcp_error)))
                    .ok();
            }
        }
    }

    fn handle_notification(inner: &std::sync::Arc<Inner>, method: &str, params: Option<&Value>) {
        if method == "notifications/progress" {
            Self::handle_progress(inner, params);
        } else if method == "notifications/cancelled" {
            Self::handle_cancelled(inner, params);
        }
        let listeners: Vec<NotificationListener> = inner
            .notification_listeners
            .lock()
            .expect("listeners cannot be poisoned")
            .iter()
            .filter(|(existing, _, _)| existing == method)
            .map(|(_, _, listener)| std::sync::Arc::clone(listener))
            .collect();
        let default_params = Value::Object(serde_json::Map::new());
        let params = params.unwrap_or(&default_params);
        for listener in listeners {
            // Upstream try/catch: a throwing listener surfaces on the error
            // listeners and does not break the loop.
            let inner_for_listener = std::sync::Arc::clone(inner);
            let params_for_listener = params.clone();
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                listener(&params_for_listener);
            }))
            .unwrap_or_else(|panic| {
                let message = panic_message(panic);
                Self::emit_error(&inner_for_listener, &McpClientError::Other(message));
            });
        }
    }

    fn handle_progress(inner: &std::sync::Arc<Inner>, params: Option<&Value>) {
        let Some(object) = params.and_then(Value::as_object) else {
            return;
        };
        let Some(token) = object.get("progressToken").and_then(JsonRpcId::from_value) else {
            return;
        };
        let Some(progress) = object.get("progress").and_then(Value::as_f64) else {
            return;
        };
        let _ = progress;
        let request_id = inner
            .progress_requests
            .lock()
            .expect("progress map cannot be poisoned")
            .get(&token)
            .cloned();
        let Some(request_id) = request_id else {
            return;
        };
        // Progress re-arms the timeout (upstream armTimeout); the listener
        // call happens outside the lock so it can re-enter the client.
        let (listener, payload) = {
            let mut pending = inner.pending.lock().expect("pending cannot be poisoned");
            let Some(entry) = pending.get_mut(&request_id) else {
                return;
            };
            if let Some(deadline_cell) = &entry.deadline {
                deadline_cell.set(Some(
                    Instant::now() + Duration::from_millis(entry.timeout_ms),
                ));
            }
            let payload = params.expect("checked above").clone();
            match &entry.on_progress {
                Some(on_progress) => (Some(std::sync::Arc::clone(on_progress)), payload),
                None => (None, payload),
            }
        };
        if let Some(listener) = listener {
            let inner_for_listener = std::sync::Arc::clone(inner);
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                listener(&payload);
            }))
            .unwrap_or_else(|panic| {
                let message = panic_message(panic);
                Self::emit_error(&inner_for_listener, &McpClientError::Other(message));
            });
        }
    }

    fn handle_cancelled(inner: &std::sync::Arc<Inner>, params: Option<&Value>) {
        let Some(object) = params.and_then(Value::as_object) else {
            return;
        };
        let Some(request_id) = object.get("requestId").and_then(JsonRpcId::from_value) else {
            return;
        };
        if let Some(controller) = inner
            .incoming
            .lock()
            .expect("incoming cannot be poisoned")
            .get(&request_id)
        {
            controller.cancel();
        }
    }

    // -- pagination ---------------------------------------------------------

    async fn list_page(
        &self,
        method: &str,
        key: &str,
        is_item: fn(&serde_json::Map<String, Value>) -> bool,
        cursor: Option<String>,
        options: &McpRequestOptions,
    ) -> Result<ListPage, McpClientError> {
        let params = cursor.map(|cursor| {
            let mut params = serde_json::Map::new();
            params.insert("cursor".into(), Value::from(cursor));
            params
        });
        let result = self.request(method, params, options.clone()).await?;
        validate_list_page(method, key, &result, is_item)
    }

    /// Every item of a paginated list method (upstream `listAll`).
    async fn list_all(
        &self,
        method: &str,
        key: &str,
        is_item: fn(&serde_json::Map<String, Value>) -> bool,
        options: McpRequestOptions,
    ) -> Result<Vec<serde_json::Map<String, Value>>, McpClientError> {
        let mut items = Vec::new();
        let mut cursors = std::collections::HashSet::new();
        let mut cursor: Option<String> = None;
        for _page_number in 0..MAX_LIST_PAGES {
            let page = self
                .list_page(method, key, is_item, cursor.clone(), &options)
                .await?;
            items.extend(page.items);
            let Some(next_cursor) = page.next_cursor else {
                return Ok(items);
            };
            if cursors.contains(&next_cursor) {
                return Err(McpClientError::Other(format!(
                    "MCP {method} returned duplicate cursor: {next_cursor}"
                )));
            }
            cursors.insert(next_cursor.clone());
            cursor = Some(next_cursor);
        }
        Err(McpClientError::Other(format!(
            "MCP {method} exceeded {MAX_LIST_PAGES} pages"
        )))
    }
}

struct ListPage {
    items: Vec<serde_json::Map<String, Value>>,
    next_cursor: Option<String>,
    meta: Option<serde_json::Map<String, Value>>,
}

struct TransportPump {
    client: std::sync::Arc<Inner>,
}

impl TransportPump {
    /// Upstream `handleMessage`: response, then request, then notification;
    /// anything else surfaces an invalid-message error.
    fn handle_message(&self, message: &JsonRpcMessage) {
        match message {
            JsonRpcMessage::Response(_) => McpClient::handle_response(
                &self.client,
                match message {
                    JsonRpcMessage::Response(response) => response.clone(),
                    _ => unreachable!(),
                },
            ),
            JsonRpcMessage::Request { .. } => {
                let inner = std::sync::Arc::clone(&self.client);
                let parsed = message.clone();
                tokio::spawn(async move {
                    if let JsonRpcMessage::Request { id, method, params } = parsed {
                        Self::handle_request(&inner, id, method, params).await;
                    }
                });
            }
            JsonRpcMessage::Notification { method, params } => {
                McpClient::handle_notification(&self.client, method, params.as_ref());
            }
        }
    }

    /// Upstream `handleRequest`: dispatch to the registered handler and send
    /// the success or error response.
    async fn handle_request(
        inner: &std::sync::Arc<Inner>,
        id: JsonRpcId,
        method: String,
        params: Option<Value>,
    ) {
        let Some(transport) = inner
            .transport
            .lock()
            .expect("transport cannot be poisoned")
            .clone()
        else {
            return;
        };
        let handler = inner
            .request_handlers
            .lock()
            .expect("handlers cannot be poisoned")
            .iter()
            .find(|(existing, _)| *existing == method)
            .map(|(_, handler)| std::sync::Arc::clone(handler));
        let Some(handler) = handler else {
            let error = JsonRpcMessage::Response(JsonRpcResponse::Error {
                id: id.clone(),
                error: crate::mcp::protocol::jsonrpc::JsonRpcErrorObject::new(
                    JSON_RPC_ERROR_CODES_METHOD_NOT_FOUND,
                    format!("Method not found: {method}"),
                ),
            });
            if let Err(send_error) = transport.send(&error).await {
                McpClient::emit_error(inner, &send_error);
            }
            return;
        };
        let controller = CancellationToken::new();
        inner
            .incoming
            .lock()
            .expect("incoming cannot be poisoned")
            .insert(id.clone(), controller.clone());
        let context = RequestContext { signal: controller };
        let result = handler(params, context).await;
        let response = match result {
            Ok(result) => JsonRpcMessage::Response(JsonRpcResponse::Success {
                id,
                // Upstream `result ?? {}`.
                result: if result.is_null() {
                    serde_json::json!({})
                } else {
                    result
                },
            }),
            Err(error) => JsonRpcMessage::Response(JsonRpcResponse::Error {
                id,
                error: match error {
                    McpClientError::Mcp(mcp_error) => {
                        let mut error_object =
                            crate::mcp::protocol::jsonrpc::JsonRpcErrorObject::new(
                                mcp_error.code,
                                mcp_error.message,
                            );
                        if let Some(data) = mcp_error.data {
                            error_object = error_object.with_data(data);
                        }
                        error_object
                    }
                    other => crate::mcp::protocol::jsonrpc::JsonRpcErrorObject::new(
                        JSON_RPC_ERROR_CODES_INTERNAL_ERROR,
                        other.to_string(),
                    ),
                },
            }),
        };
        // Upstream `finally { this.incoming.delete(message.id); }` runs after
        // the response send is attempted.
        let send_result = transport.send(&response).await;
        inner
            .incoming
            .lock()
            .expect("incoming cannot be poisoned")
            .remove(response_id(&response));
        if let Err(send_error) = send_result {
            McpClient::emit_error(inner, &send_error);
        }
    }
}

fn response_id(message: &JsonRpcMessage) -> &JsonRpcId {
    match message {
        JsonRpcMessage::Response(response) => response.id(),
        _ => unreachable!("only responses reach response_id"),
    }
}

fn panic_message(panic: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = panic.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else {
        "handler panicked".to_string()
    }
}

// -- validation -------------------------------------------------------------

/// Upstream `validateInitializeResult`.
fn validate_initialize_result(value: &Value) -> Result<InitializeResult, McpClientError> {
    let invalid = || {
        McpClientError::Mcp(McpError::new(
            JSON_RPC_ERROR_CODES_INVALID_REQUEST,
            "Invalid MCP initialize result",
        ))
    };
    let Some(object) = value.as_object() else {
        return Err(invalid());
    };
    if !object.get("protocolVersion").is_some_and(Value::is_string)
        || !object.get("capabilities").is_some_and(Value::is_object)
        || !object.get("serverInfo").is_some_and(Value::is_object)
    {
        return Err(invalid());
    }
    let server_info = object
        .get("serverInfo")
        .and_then(Value::as_object)
        .expect("checked");
    if !server_info.get("name").is_some_and(Value::is_string)
        || !server_info.get("version").is_some_and(Value::is_string)
    {
        return Err(invalid());
    }
    // `instructions !== undefined && typeof instructions !== "string"`:
    // JSON null fails the string check.
    if let Some(instructions) = object.get("instructions") {
        if !instructions.is_string() {
            return Err(invalid());
        }
    }
    serde_json::from_value(value.clone()).map_err(|_| invalid())
}

fn invalid(message: impl Into<String>) -> McpClientError {
    McpClientError::Mcp(McpError::new(JSON_RPC_ERROR_CODES_INVALID_REQUEST, message))
}

fn into_invalid(error: serde_json::Error) -> McpClientError {
    invalid(error.to_string())
}

/// Upstream `validateListPage`.
fn validate_list_page(
    method: &str,
    key: &str,
    value: &Value,
    is_item: fn(&serde_json::Map<String, Value>) -> bool,
) -> Result<ListPage, McpClientError> {
    let Some(object) = value.as_object() else {
        return Err(invalid(format!("Invalid MCP {method} result")));
    };
    let Some(items) = object.get(key).and_then(Value::as_array) else {
        return Err(invalid(format!("Invalid MCP {method} result")));
    };
    for item in items {
        let Some(item) = item.as_object() else {
            return Err(invalid(format!("Invalid entry in MCP {method} result")));
        };
        if !is_item(item) {
            return Err(invalid(format!("Invalid entry in MCP {method} result")));
        }
    }
    // Some servers (v1.0.0) end pagination with `null` or `""` instead of
    // omitting the cursor; both count as absent.
    let next_cursor = match object.get("nextCursor") {
        None | Some(Value::Null) => None,
        Some(Value::String(cursor)) if cursor.is_empty() => None,
        Some(Value::String(cursor)) => Some(cursor.clone()),
        Some(_) => return Err(invalid(format!("Invalid MCP {method} cursor"))),
    };
    Ok(ListPage {
        items: items
            .iter()
            .map(|item| item.as_object().expect("checked").clone())
            .collect(),
        next_cursor,
        meta: object.get("_meta").and_then(Value::as_object).cloned(),
    })
}

fn is_tool_value(tool: &serde_json::Map<String, Value>) -> bool {
    tool.get("name").is_some_and(Value::is_string)
        && tool.get("inputSchema").is_some_and(Value::is_object)
}

/// `name` is required by the spec, but some servers omit it; the URI stands
/// in (upstream `toResource`).
fn is_resource_value(resource: &serde_json::Map<String, Value>) -> bool {
    resource.get("uri").is_some_and(Value::is_string)
        && resource.get("name").is_none_or(|name| name.is_string())
}

fn is_resource_template_value(template: &serde_json::Map<String, Value>) -> bool {
    template.get("uriTemplate").is_some_and(Value::is_string)
        && template.get("name").is_none_or(|name| name.is_string())
}

/// Upstream `toResource`: `{ ...item, name: item.name ?? item.uri }` — the
/// insert replaces an existing `name` key in position or appends at the end.
fn to_resource(item: &serde_json::Map<String, Value>) -> serde_json::Map<String, Value> {
    let mut patched = item.clone();
    let name = item
        .get("name")
        .filter(|name| !name.is_null())
        .cloned()
        .unwrap_or_else(|| item.get("uri").cloned().unwrap_or(Value::Null));
    patched.insert("name".into(), name);
    patched
}

fn to_resource_template(item: &serde_json::Map<String, Value>) -> serde_json::Map<String, Value> {
    let mut patched = item.clone();
    let name = item
        .get("name")
        .filter(|name| !name.is_null())
        .cloned()
        .unwrap_or_else(|| item.get("uriTemplate").cloned().unwrap_or(Value::Null));
    patched.insert("name".into(), name);
    patched
}

/// Upstream `validateReadResourceResult`.
fn validate_read_resource_result(value: &Value) -> Result<ReadResourceResult, McpClientError> {
    let Some(object) = value.as_object() else {
        return Err(invalid("Invalid MCP resources/read result"));
    };
    let Some(contents) = object.get("contents").and_then(Value::as_array) else {
        return Err(invalid("Invalid MCP resources/read result"));
    };
    for content in contents {
        let valid = content.as_object().is_some_and(|content| {
            content.get("uri").is_some_and(Value::is_string)
                && (content.get("text").is_some_and(Value::is_string)
                    || content.get("blob").is_some_and(Value::is_string))
        });
        if !valid {
            return Err(invalid("Invalid contents in MCP resources/read result"));
        }
    }
    serde_json::from_value(value.clone()).map_err(into_invalid)
}

/// Upstream `validateCallToolResult`: `content` defaults to `[]` for servers
/// that only return `structuredContent`.
fn validate_call_tool_result(value: &Value) -> Result<CallToolResult, McpClientError> {
    let invalid_result = || invalid("Invalid MCP tools/call result");
    let Some(object) = value.as_object() else {
        return Err(invalid_result());
    };
    if let Some(content) = object.get("content") {
        if !content.is_array() {
            return Err(invalid_result());
        }
    }
    if let Some(structured) = object.get("structuredContent") {
        if !structured.is_object() {
            return Err(invalid("Invalid MCP tools/call structured content"));
        }
    }
    let mut normalized = object.clone();
    if !normalized.contains_key("content") {
        normalized.insert("content".into(), Value::Array(Vec::new()));
    }
    serde_json::from_value(Value::Object(normalized)).map_err(into_invalid)
}
