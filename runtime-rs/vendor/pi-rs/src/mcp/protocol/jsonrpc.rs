//! JSON-RPC 2.0 message model for the MCP client, ported from upstream
//! `packages/mcp/src/protocol/jsonrpc.ts` (the `@earendil-works/pi-mcp`
//! package at 2bbfcca43).
//!
//! Messages are dynamic [`serde_json`] values (upstream types `params` and
//! `result` as `unknown`), serialized with the crate-wide `preserve_order`
//! feature so constructed messages keep the upstream object key order:
//! requests `{jsonrpc, id, method, params?}`, notifications
//! `{jsonrpc, method, params?}`, success responses `{jsonrpc, id, result}`,
//! error responses `{jsonrpc, id, error}` with `{code, message, data?}`
//! (the `data` key is omitted when absent, exactly like
//! `JSON.stringify` dropping `data: undefined`).
//!
//! Parser predicate order is load-bearing and matches upstream
//! [`parse_json_rpc_message`]: request first, then notification, then
//! response — e.g. `{"jsonrpc":"2.0","id":1,"method":"m"}` is a request even
//! though it would also satisfy the response shape checks.

use std::fmt;

use serde::ser::{Serialize, Serializer};
use serde_json::Value;

/// Upstream `JsonRpcId` (`string | number` with finite numbers). Numeric ids
/// are canonicalized on parse the way JavaScript numbers work: an integral
/// float (`2.0`) parses to `2`, so client/server id comparison and
/// re-serialization match `JSON.parse`/`JSON.stringify` round trips.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum JsonRpcId {
    String(String),
    Number(serde_json::Number),
}

impl JsonRpcId {
    /// Upstream `isJsonRpcId`: a string or a finite number. Integral JSON
    /// floats are canonicalized to integers (JavaScript has one number type;
    /// `JSON.parse("2.0") === 2`).
    pub fn from_value(value: &Value) -> Option<JsonRpcId> {
        match value {
            Value::String(text) => Some(JsonRpcId::String(text.clone())),
            Value::Number(number) => Some(JsonRpcId::Number(canonical_number(number))),
            _ => None,
        }
    }

    /// Upstream `String(id)` for error messages.
    pub fn to_display_string(&self) -> String {
        match self {
            JsonRpcId::String(text) => text.clone(),
            JsonRpcId::Number(number) => number.to_string(),
        }
    }
}

impl fmt::Display for JsonRpcId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.to_display_string())
    }
}

/// `JSON.parse("2.0") === 2`: integral doubles become integers so equality
/// and `JSON.stringify` output match JavaScript.
pub(crate) fn canonical_number(number: &serde_json::Number) -> serde_json::Number {
    if let Some(float) = number.as_f64() {
        if float.fract() == 0.0
            && float.abs() <= 9.007_199_254_740_992e15
            && float >= i64::MIN as f64
            && float <= i64::MAX as f64
        {
            return serde_json::Number::from(float as i64);
        }
    }
    number.clone()
}

/// Upstream `JsonRpcRequest` / `JsonRpcNotification` / `JsonRpcResponse`.
/// `params`/`result` stay raw values; serialization order is fixed below.
#[derive(Debug, Clone)]
pub enum JsonRpcMessage {
    Request {
        id: JsonRpcId,
        method: String,
        params: Option<Value>,
    },
    Notification {
        method: String,
        params: Option<Value>,
    },
    Response(JsonRpcResponse),
}

#[derive(Debug, Clone)]
pub enum JsonRpcResponse {
    Success {
        id: JsonRpcId,
        result: Value,
    },
    Error {
        id: JsonRpcId,
        error: JsonRpcErrorObject,
    },
}

impl JsonRpcResponse {
    pub fn id(&self) -> &JsonRpcId {
        match self {
            JsonRpcResponse::Success { id, .. } | JsonRpcResponse::Error { id, .. } => id,
        }
    }
}

/// Upstream `JsonRpcErrorObject`. `data: None` serializes without the key.
#[derive(Debug, Clone, PartialEq)]
pub struct JsonRpcErrorObject {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl JsonRpcErrorObject {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        JsonRpcErrorObject {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    pub fn to_value(&self) -> Value {
        let mut object = serde_json::Map::new();
        object.insert("code".into(), Value::from(self.code));
        object.insert("message".into(), Value::from(self.message.clone()));
        if let Some(data) = &self.data {
            object.insert("data".into(), data.clone());
        }
        Value::Object(object)
    }
}

fn string_value(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str)
}

/// Upstream `isJsonRpcRequest`: object with `jsonrpc: "2.0"`, a valid id, and
/// a string method. Does not reject extra keys (e.g. a `result` field).
pub fn is_json_rpc_request(message: &Value) -> bool {
    let Some(object) = message.as_object() else {
        return false;
    };
    string_value(object.get("jsonrpc")) == Some("2.0")
        && object
            .get("id")
            .is_some_and(|id| JsonRpcId::from_value(id).is_some())
        && object.get("method").is_some_and(Value::is_string)
}

/// Upstream `isJsonRpcNotification`: object with `jsonrpc: "2.0"`, no `id`
/// key at all, and a string method.
pub fn is_json_rpc_notification(message: &Value) -> bool {
    let Some(object) = message.as_object() else {
        return false;
    };
    string_value(object.get("jsonrpc")) == Some("2.0")
        && !object.contains_key("id")
        && object.get("method").is_some_and(Value::is_string)
}

/// Upstream `isJsonRpcResponse`: object with `jsonrpc: "2.0"` and a valid id;
/// either `result` present without `error`, or an `error` object with a
/// numeric `code` and string `message`.
pub fn is_json_rpc_response(message: &Value) -> bool {
    let Some(object) = message.as_object() else {
        return false;
    };
    if string_value(object.get("jsonrpc")) != Some("2.0") {
        return false;
    }
    let Some(id) = object.get("id") else {
        return false;
    };
    if JsonRpcId::from_value(id).is_none() {
        return false;
    }
    if object.contains_key("result") {
        return !object.contains_key("error");
    }
    let Some(error) = object.get("error") else {
        return false;
    };
    let Some(error_object) = error.as_object() else {
        return false;
    };
    error_object.get("code").is_some_and(Value::is_number)
        && error_object.get("message").is_some_and(Value::is_string)
}

/// Upstream `parseJsonRpcMessage`: request, then notification, then response.
/// Anything else throws `McpError(-32600, "Invalid JSON-RPC message")`.
pub fn parse_json_rpc_message(value: &Value) -> Result<JsonRpcMessage, McpError> {
    let Some(object) = value.as_object() else {
        return Err(invalid_request_message());
    };
    if is_json_rpc_request(value) {
        return Ok(JsonRpcMessage::Request {
            id: JsonRpcId::from_value(object.get("id").expect("checked")).expect("checked"),
            method: object
                .get("method")
                .and_then(Value::as_str)
                .expect("checked")
                .to_string(),
            params: object.get("params").cloned(),
        });
    }
    if is_json_rpc_notification(value) {
        return Ok(JsonRpcMessage::Notification {
            method: object
                .get("method")
                .and_then(Value::as_str)
                .expect("checked")
                .to_string(),
            params: object.get("params").cloned(),
        });
    }
    if is_json_rpc_response(value) {
        let id = JsonRpcId::from_value(object.get("id").expect("checked")).expect("checked");
        if let Some(result) = object.get("result") {
            return Ok(JsonRpcMessage::Response(JsonRpcResponse::Success {
                id,
                result: result.clone(),
            }));
        }
        let error = object
            .get("error")
            .and_then(Value::as_object)
            .expect("checked");
        return Ok(JsonRpcMessage::Response(JsonRpcResponse::Error {
            id,
            error: JsonRpcErrorObject {
                code: error
                    .get("code")
                    .and_then(Value::as_f64)
                    .map(|code| code as i64)
                    .expect("checked"),
                message: error
                    .get("message")
                    .and_then(Value::as_str)
                    .expect("checked")
                    .to_string(),
                data: error.get("data").cloned(),
            },
        }));
    }
    Err(invalid_request_message())
}

fn invalid_request_message() -> McpError {
    McpError::new(
        JSON_RPC_ERROR_CODES_INVALID_REQUEST,
        "Invalid JSON-RPC message",
    )
}

impl JsonRpcMessage {
    /// The exact `JSON.stringify` of the message: key order per shape, with
    /// `params`/`result`/`error` omitted when absent (upstream conditional
    /// spreads; `JSON.stringify` drops `undefined` values).
    pub fn to_json_string(&self) -> String {
        serde_json::to_string(self).expect("JsonRpcMessage serialization cannot fail")
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("JsonRpcMessage serialization cannot fail")
    }
}

impl Serialize for JsonRpcMessage {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error;
        let value = match self {
            JsonRpcMessage::Request { id, method, params } => {
                let mut object = serde_json::Map::new();
                object.insert("jsonrpc".into(), Value::from("2.0"));
                object.insert("id".into(), id_value(id));
                object.insert("method".into(), Value::from(method.clone()));
                if let Some(params) = params {
                    object.insert("params".into(), params.clone());
                }
                Value::Object(object)
            }
            JsonRpcMessage::Notification { method, params } => {
                let mut object = serde_json::Map::new();
                object.insert("jsonrpc".into(), Value::from("2.0"));
                object.insert("method".into(), Value::from(method.clone()));
                if let Some(params) = params {
                    object.insert("params".into(), params.clone());
                }
                Value::Object(object)
            }
            JsonRpcMessage::Response(JsonRpcResponse::Success { id, result }) => {
                let mut object = serde_json::Map::new();
                object.insert("jsonrpc".into(), Value::from("2.0"));
                object.insert("id".into(), id_value(id));
                object.insert("result".into(), result.clone());
                Value::Object(object)
            }
            JsonRpcMessage::Response(JsonRpcResponse::Error { id, error }) => {
                let mut object = serde_json::Map::new();
                object.insert("jsonrpc".into(), Value::from("2.0"));
                object.insert("id".into(), id_value(id));
                object.insert("error".into(), error.to_value());
                Value::Object(object)
            }
        };
        Value::serialize(&value, serializer).map_err(|error| S::Error::custom(error.to_string()))
    }
}

fn id_value(id: &JsonRpcId) -> Value {
    match id {
        JsonRpcId::String(text) => Value::from(text.clone()),
        JsonRpcId::Number(number) => Value::Number(number.clone()),
    }
}

/// Upstream `JSON_RPC_ERROR_CODES`.
pub const JSON_RPC_ERROR_CODES_PARSE_ERROR: i64 = -32700;
pub const JSON_RPC_ERROR_CODES_INVALID_REQUEST: i64 = -32600;
pub const JSON_RPC_ERROR_CODES_METHOD_NOT_FOUND: i64 = -32601;
pub const JSON_RPC_ERROR_CODES_INVALID_PARAMS: i64 = -32602;
pub const JSON_RPC_ERROR_CODES_INTERNAL_ERROR: i64 = -32603;

/// Upstream `McpError`: a JSON-RPC error with code and optional data.
#[derive(Debug, Clone, PartialEq)]
pub struct McpError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl McpError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        McpError {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }
}

impl fmt::Display for McpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for McpError {}

/// Unified error surface of the client and transports, standing in for the
/// upstream error classes: `McpError`, `McpConnectionClosedError`,
/// `McpTimeoutError`, `McpAbortError`, `McpHttpError` and subclasses, and the
/// plain `Error` objects (network failures, lifecycle checks).
#[derive(Debug, Clone)]
pub enum McpClientError {
    /// `McpError` (a JSON-RPC or protocol error with a numeric code).
    Mcp(McpError),
    /// `McpConnectionClosedError` (default message "MCP connection closed").
    ConnectionClosed(String),
    /// `McpTimeoutError`; carries `timeoutMs`.
    Timeout(u64),
    /// `McpAbortError` (named "AbortError" upstream).
    Aborted(String),
    /// `McpHttpError` (status + message + response body).
    Http(McpHttpError),
    /// `McpAuthRequiredError` (401).
    AuthRequired(McpAuthRequiredError),
    /// `McpSessionExpiredError` (404 with a session).
    SessionExpired(McpSessionExpiredError),
    /// Upstream `TypeError` from `fetch` — a network-level failure.
    Network(String),
    /// OAuth flow errors surfacing through the adapted auth provider
    /// (`adaptOAuthProvider`), including `McpOAuthAuthorizationRequiredError`.
    OAuth(crate::mcp::oauth::errors::OAuthFlowError),
    /// Any other plain `Error`, normalized with `String(error)`.
    Other(String),
}

impl McpClientError {
    /// Upstream `error.message`.
    pub fn message(&self) -> std::borrow::Cow<'_, str> {
        match self {
            McpClientError::Mcp(error) => error.message.as_str().into(),
            McpClientError::ConnectionClosed(message)
            | McpClientError::Aborted(message)
            | McpClientError::Network(message)
            | McpClientError::Other(message) => message.as_str().into(),
            McpClientError::Timeout(_) => "timeout".into(),
            McpClientError::Http(error) => error.message.as_str().into(),
            McpClientError::AuthRequired(_) => "MCP server requires authentication".into(),
            McpClientError::SessionExpired(_) => "MCP session expired".into(),
            McpClientError::OAuth(error) => error.to_string().into(),
        }
    }
}

impl fmt::Display for McpClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            McpClientError::Mcp(error) => write!(formatter, "{}", error.message),
            McpClientError::ConnectionClosed(message)
            | McpClientError::Aborted(message)
            | McpClientError::Network(message)
            | McpClientError::Other(message) => write!(formatter, "{message}"),
            McpClientError::Timeout(timeout_ms) => {
                write!(formatter, "MCP request timed out after {timeout_ms}ms")
            }
            McpClientError::Http(error) => write!(formatter, "{}", error.message),
            McpClientError::AuthRequired(_) => {
                write!(formatter, "MCP server requires authentication")
            }
            McpClientError::SessionExpired(_) => write!(formatter, "MCP session expired"),
            McpClientError::OAuth(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for McpClientError {}

impl From<McpError> for McpClientError {
    fn from(error: McpError) -> Self {
        McpClientError::Mcp(error)
    }
}

/// Upstream `McpHttpError`.
#[derive(Debug, Clone, PartialEq)]
pub struct McpHttpError {
    pub status: u16,
    pub message: String,
    pub body: String,
}

impl McpHttpError {
    pub fn new(status: u16, message: impl Into<String>, body: impl Into<String>) -> Self {
        McpHttpError {
            status,
            message: message.into(),
            body: body.into(),
        }
    }
}

/// Upstream `McpAuthRequiredError` (a 401 `McpHttpError` carrying the
/// `www-authenticate` challenge).
#[derive(Debug, Clone, PartialEq)]
pub struct McpAuthRequiredError {
    pub body: String,
    pub www_authenticate: Option<String>,
}

/// Upstream `McpSessionExpiredError` (404 while a session id is set).
#[derive(Debug, Clone, PartialEq)]
pub struct McpSessionExpiredError {
    pub body: String,
}

/// Helper constructors matching the upstream `new` signatures.
impl McpClientError {
    pub fn connection_closed() -> Self {
        McpClientError::ConnectionClosed("MCP connection closed".into())
    }

    pub fn connection_closed_with(message: impl Into<String>) -> Self {
        McpClientError::ConnectionClosed(message.into())
    }

    pub fn timeout(timeout_ms: u64) -> Self {
        McpClientError::Timeout(timeout_ms)
    }

    pub fn aborted() -> Self {
        McpClientError::Aborted("MCP request aborted".into())
    }
}

// -- named upstream error classes -------------------------------------------
//
// Upstream models request failures as distinct `Error` subclasses; the port
// carries them in [`McpClientError`] at runtime (Display strings identical),
// and these named types mirror the class surface for consumers.

/// Upstream `McpConnectionClosedError` (default message "MCP connection
/// closed").
#[derive(Debug, Clone, PartialEq)]
pub struct McpConnectionClosedError(pub String);

impl McpConnectionClosedError {
    pub fn new() -> Self {
        McpConnectionClosedError("MCP connection closed".to_string())
    }
}

impl Default for McpConnectionClosedError {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for McpConnectionClosedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl std::error::Error for McpConnectionClosedError {}

impl From<McpConnectionClosedError> for McpClientError {
    fn from(error: McpConnectionClosedError) -> Self {
        McpClientError::ConnectionClosed(error.0)
    }
}

/// Upstream `McpTimeoutError`.
#[derive(Debug, Clone, PartialEq)]
pub struct McpTimeoutError {
    pub timeout_ms: u64,
}

impl McpTimeoutError {
    pub fn new(timeout_ms: u64) -> Self {
        McpTimeoutError { timeout_ms }
    }
}

impl fmt::Display for McpTimeoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "MCP request timed out after {}ms",
            self.timeout_ms
        )
    }
}

impl std::error::Error for McpTimeoutError {}

impl From<McpTimeoutError> for McpClientError {
    fn from(error: McpTimeoutError) -> Self {
        McpClientError::Timeout(error.timeout_ms)
    }
}

/// Upstream `McpAbortError` (named "AbortError" upstream).
#[derive(Debug, Clone, PartialEq)]
pub struct McpAbortError(pub String);

impl McpAbortError {
    pub fn new() -> Self {
        McpAbortError("MCP request aborted".to_string())
    }
}

impl Default for McpAbortError {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for McpAbortError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl std::error::Error for McpAbortError {}

impl From<McpAbortError> for McpClientError {
    fn from(error: McpAbortError) -> Self {
        McpClientError::Aborted(error.0)
    }
}
