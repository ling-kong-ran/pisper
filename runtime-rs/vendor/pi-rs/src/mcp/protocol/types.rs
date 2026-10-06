//! MCP protocol surface types, ported from upstream
//! `packages/mcp/src/protocol/types.ts`.
//!
//! The upstream interfaces are structural over server-provided JSON, so the
//! port models them as serde structs with `#[serde(flatten)]` extra maps:
//! every captured key and value survives, and unknown future fields are kept
//! (the pinned contract for re-serialized server items is the key SET plus
//! every value, per the ai-delta precedent for serde map ordering).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Upstream `LATEST_PROTOCOL_VERSION`.
pub const LATEST_PROTOCOL_VERSION: &str = "2025-11-25";

/// Versions the client accepts from a server. Servers that do not support the
/// requested version answer with their own latest one, so older versions stay
/// accepted for servers built on older SDKs.
pub const SUPPORTED_PROTOCOL_VERSIONS: [&str; 4] =
    ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// Upstream `Implementation` (`clientInfo` / `serverInfo`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Implementation {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl Implementation {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Implementation {
            name: name.into(),
            version: version.into(),
            title: None,
            extra: serde_json::Map::new(),
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }
}

/// Upstream `Root`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Root {
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// Upstream `ClientCapabilities` (`roots` / `sampling` / `elicitation` /
/// `experimental`). The client serializes the caller-provided capabilities
/// object verbatim; this struct mirrors the surface for consumers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ClientCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experimental: Option<serde_json::Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roots: Option<RightsCapabilityEmpty>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampling: Option<serde_json::Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elicitation: Option<serde_json::Map<String, Value>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// Upstream `roots: { listChanged?: boolean }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RightsCapabilityEmpty {
    #[serde(
        rename = "listChanged",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub list_changed: Option<bool>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// Upstream `ServerCapabilities` (the initialize result's capabilities).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ServerCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experimental: Option<serde_json::Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logging: Option<serde_json::Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompts: Option<RightsCapabilityEmpty>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourcesCapability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<RightsCapabilityEmpty>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completions: Option<serde_json::Map<String, Value>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// Upstream `resources: { subscribe?: boolean; listChanged?: boolean }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ResourcesCapability {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscribe: Option<bool>,
    #[serde(
        rename = "listChanged",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub list_changed: Option<bool>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// Upstream `InitializeResult`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    pub capabilities: serde_json::Map<String, Value>,
    #[serde(rename = "serverInfo")]
    pub server_info: Implementation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// Upstream `ProgressNotification` — the raw `notifications/progress` params.
/// Progress payloads are forwarded as raw values (upstream passes
/// `params as unknown as ProgressNotification` after validating the token and
/// numeric progress), so the client hands listeners the JSON object as-is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgressNotification {
    #[serde(rename = "progressToken")]
    pub progress_token: Value,
    pub progress: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// Upstream `CancelledNotification`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CancelledNotification {
    #[serde(rename = "requestId")]
    pub request_id: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Upstream `ToolAnnotations`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ToolAnnotations {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(
        rename = "readOnlyHint",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub read_only_hint: Option<bool>,
    #[serde(
        rename = "destructiveHint",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub destructive_hint: Option<bool>,
    #[serde(
        rename = "idempotentHint",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub idempotent_hint: Option<bool>,
    #[serde(
        rename = "openWorldHint",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub open_world_hint: Option<bool>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// Upstream `ToolExecution`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ToolExecution {
    #[serde(
        rename = "taskSupport",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub task_support: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// Upstream `Tool`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Map<String, Value>,
    #[serde(
        rename = "outputSchema",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub output_schema: Option<serde_json::Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ToolAnnotations>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<ToolExecution>,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Map<String, Value>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// Upstream `ListToolsResult`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListToolsResult {
    pub tools: Vec<Tool>,
    #[serde(
        rename = "nextCursor",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub next_cursor: Option<String>,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Map<String, Value>>,
}

/// Upstream `Resource` (a resource listed by `resources/list`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resource {
    pub uri: String,
    /// Required by the spec, but some servers omit it; [`crate::mcp`] fills
    /// it with the URI like upstream `toResource`.
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "mimeType", default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<serde_json::Map<String, Value>>,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Map<String, Value>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// Upstream `ResourceTemplate` (an RFC 6570 URI template family from
/// `resources/templates/list`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceTemplate {
    #[serde(rename = "uriTemplate")]
    pub uri_template: String,
    /// Required by the spec, but some servers omit it; filled with the URI
    /// template like upstream `toResourceTemplate`.
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "mimeType", default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<serde_json::Map<String, Value>>,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Map<String, Value>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// Upstream `ListResourcesResult`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListResourcesResult {
    pub resources: Vec<Resource>,
    #[serde(
        rename = "nextCursor",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub next_cursor: Option<String>,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Map<String, Value>>,
}

/// Upstream `ListResourceTemplatesResult`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListResourceTemplatesResult {
    #[serde(rename = "resourceTemplates")]
    pub resource_templates: Vec<ResourceTemplate>,
    #[serde(
        rename = "nextCursor",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub next_cursor: Option<String>,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Map<String, Value>>,
}

/// Upstream `ReadResourceResult`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReadResourceResult {
    pub contents: Vec<serde_json::Map<String, Value>>,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Map<String, Value>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}
