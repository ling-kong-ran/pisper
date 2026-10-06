//! Port of upstream `coding-agent/src/extensions/mcp/resources.ts` (HEAD
//! `2bbfcca43`): MCP resources, through the tools Codex and opencode use:
//! `list_mcp_resources`, `list_mcp_resource_templates`, and
//! `read_mcp_resource`. They take a `server` argument and cover every
//! connected server with resources, so models trained on those tools use them
//! unchanged.
//!
//! Listings are JSON, as in Codex:
//! `{ server?, resources: [{ server, ...resource }], nextCursor? }`. With a
//! `server`, one page is listed and `cursor` continues it; without, every
//! page of every server. MCP App resources (`ui://` URIs and
//! `profile=mcp-app` HTML) are left out, since they are user interfaces for
//! hosts that render them, and so are icons. Read resources become text and
//! images for the model; binary resources are saved to temp files. Scripts
//! get the JSON payloads.
//!
//! Disclosed seams:
//! - The `server: …, ...rest` listing spread keeps the wire order upstream;
//!   the port serializes the parsed `Resource` / `ResourceTemplate` structs
//!   (field order, extras after), byte-identical for schema-ordered server
//!   output (the oracle's shape).
//! - The multi-server sort uses a lowercase-then-exact comparison standing in
//!   for `localeCompare` (identical for ASCII names).
//! - `URL.canParse` in `extensionOf` is `url::Url::parse` (see tools).

use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::{json, Map, Value};

use crate::coding_agent::core::mcp_servers::McpExposure;
use crate::coding_agent::extensions::types::{
    AbortSignal, AgentToolResultValue, ToolAnnotations, ToolDefinition,
};
use crate::mcp::protocol::content::LlmContent;
use crate::mcp::protocol::types::{
    ListResourceTemplatesResult, ListResourcesResult, ReadResourceResult, Resource,
    ResourceTemplate,
};
use crate::mcp::McpRequestOptions;

use super::tools::{
    convert_llm_content_to_json, limit_mcp_content, to_model_content, to_tool_exposure,
    McpToolDetails, SignalForwarder,
};

pub const LIST_MCP_RESOURCES_TOOL: &str = "list_mcp_resources";
pub const LIST_MCP_RESOURCE_TEMPLATES_TOOL: &str = "list_mcp_resource_templates";
pub use super::tools::READ_MCP_RESOURCE_TOOL;

/// A connected server that offers resources (upstream `McpResourceServer`).
pub trait McpResourceServer: Send + Sync {
    fn name(&self) -> &str;
    fn timeout_ms(&self) -> u64;
    fn resources_page(
        &self,
        cursor: Option<String>,
        options: McpRequestOptions,
    ) -> BoxFuture<'static, Result<ListResourcesResult, String>>;
    fn resource_templates_page(
        &self,
        cursor: Option<String>,
        options: McpRequestOptions,
    ) -> BoxFuture<'static, Result<ListResourceTemplatesResult, String>>;
    fn all_resources(
        &self,
        options: McpRequestOptions,
    ) -> BoxFuture<'static, Result<Vec<Resource>, String>>;
    fn all_resource_templates(
        &self,
        options: McpRequestOptions,
    ) -> BoxFuture<'static, Result<Vec<ResourceTemplate>, String>>;
    fn read_resource(
        &self,
        uri: &str,
        options: McpRequestOptions,
    ) -> BoxFuture<'static, Result<ReadResourceResult, String>>;
}

/// Upstream `createMcpResourceToolDefinitions` options.
pub struct McpResourceToolOptions {
    pub exposure: McpExposure,
    /// The servers whose resources the tools reach, at call time.
    pub servers: Arc<dyn Fn() -> Vec<Arc<dyn McpResourceServer>> + Send + Sync>,
}

/// MCP App user interfaces, which only hosts that render them can use
/// (upstream `isMcpAppResource`: `uri.startsWith("ui://")` or the
/// `/;\s*profile\s*=\s*"?mcp-app"?/i` pattern over `mimeType`).
pub fn is_mcp_app_resource(item: &Value) -> bool {
    static PROFILE_PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let uri = item
        .get("uri")
        .or_else(|| item.get("uriTemplate"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if uri.starts_with("ui://") {
        return true;
    }
    let mime_type = item.get("mimeType").and_then(Value::as_str).unwrap_or("");
    PROFILE_PATTERN
        .get_or_init(|| {
            regex::Regex::new(r#";\s*profile\s*=\s*"?mcp-app"?"#).expect("profile pattern compiles")
        })
        .is_match(mime_type)
}

/// A listed resource or template without `_meta` and icons, tagged with its
/// server (upstream `listed`).
fn listed(server: &str, item: &Value) -> Value {
    let mut out = Map::new();
    out.insert("server".into(), Value::from(server));
    if let Some(record) = item.as_object() {
        for (key, value) in record {
            if key == "_meta" || key == "icons" {
                continue;
            }
            out.insert(key.clone(), value.clone());
        }
    }
    Value::Object(out)
}

fn list_parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "server": {"type": "string", "description": "MCP server name. Omit to list every server with resources."},
            "cursor": {"type": "string", "description": "Opaque cursor from a previous call with the same server; omit for the first page."},
        },
        "additionalProperties": false,
    })
}

fn read_parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "server": {"type": "string", "description": "MCP server name exactly as configured. Must match the 'server' field returned by list_mcp_resources."},
            "uri": {"type": "string", "description": "Resource URI to read. Must be one of the URIs returned by list_mcp_resources."},
        },
        "required": ["server", "uri"],
        "additionalProperties": false,
    })
}

fn listing_errors() -> Value {
    json!({
        "type": "array",
        "description": "Servers that could not be listed",
        "items": {
            "type": "object",
            "properties": {"server": {"type": "string"}, "error": {"type": "string"}},
            "required": ["server", "error"],
        },
    })
}

fn list_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "server": {"type": "string"},
            "resources": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "server": {"type": "string"},
                        "uri": {"type": "string"},
                        "name": {"type": "string"},
                        "title": {"type": "string"},
                        "description": {"type": "string"},
                        "mimeType": {"type": "string"},
                        "size": {"type": "number"},
                    },
                    "required": ["server", "uri", "name"],
                },
            },
            "nextCursor": {"type": "string"},
            "errors": listing_errors(),
        },
        "required": ["resources"],
    })
}

fn list_templates_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "server": {"type": "string"},
            "resourceTemplates": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "server": {"type": "string"},
                        "uriTemplate": {"type": "string", "description": "RFC 6570 URI template"},
                        "name": {"type": "string"},
                        "title": {"type": "string"},
                        "description": {"type": "string"},
                        "mimeType": {"type": "string"},
                    },
                    "required": ["server", "uriTemplate", "name"],
                },
            },
            "nextCursor": {"type": "string"},
            "errors": listing_errors(),
        },
        "required": ["resourceTemplates"],
    })
}

fn read_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "server": {"type": "string"},
            "uri": {"type": "string"},
            "contents": {
                "type": "array",
                "items": {
                    "anyOf": [
                        {
                            "type": "object",
                            "properties": {
                                "uri": {"type": "string"},
                                "mimeType": {"type": "string"},
                                "text": {"type": "string"},
                            },
                            "required": ["uri", "text"],
                        },
                        {
                            "type": "object",
                            "properties": {
                                "uri": {"type": "string"},
                                "mimeType": {"type": "string"},
                                "blob": {"type": "string", "description": "base64"},
                            },
                            "required": ["uri", "blob"],
                        },
                    ],
                },
            },
        },
        "required": ["server", "uri", "contents"],
    })
}

fn string_argument(params: &Value, key: &str) -> Result<Option<String>, String> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => {
            let trimmed = text.trim();
            Ok(if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            })
        }
        Some(_) => Err(format!("{key} must be a string")),
    }
}

fn json_result(
    tool: &str,
    server: Option<&str>,
    payload: Value,
) -> BoxFuture<'static, Result<AgentToolResultValue, String>> {
    let text = serde_json::to_string(&payload).unwrap_or_else(|_| "null".to_string());
    let details = McpToolDetails {
        server: server.unwrap_or_default().to_string(),
        tool: tool.to_string(),
        full_output_path: None,
    };
    Box::pin(async move {
        let limited = limit_mcp_content(vec![LlmContent::Text { text }], None).await?;
        let details = McpToolDetails {
            full_output_path: limited.full_output_path,
            ..details
        };
        let mut result = Map::new();
        result.insert(
            "content".into(),
            convert_llm_content_to_json(&limited.content),
        );
        result.insert("details".into(), details.to_json());
        result.insert("structuredContent".into(), payload);
        Ok(Value::Object(result))
    })
}

/// Sorting stand-in for `localeCompare` (root collation): lowercase compare,
/// ties broken by the exact bytes.
pub(crate) fn locale_compare(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

/// One page result of either listing shape.
enum PageList {
    Resources(ListResourcesResult),
    Templates(ListResourceTemplatesResult),
}

fn find_server(
    servers: &Arc<dyn Fn() -> Vec<Arc<dyn McpResourceServer>> + Send + Sync>,
    name: &str,
) -> Result<Arc<dyn McpResourceServer>, String> {
    let list = servers();
    let server = list.iter().find(|candidate| candidate.name() == name);
    if let Some(server) = server {
        return Ok(Arc::clone(server));
    }
    let available = list
        .iter()
        .map(|candidate| candidate.name().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    if available.is_empty() {
        return Err(format!("MCP server \"{name}\" has no resources"));
    }
    Err(format!(
        "MCP server \"{name}\" has no resources. Servers with resources: {available}"
    ))
}

/// One page of one server, or every page of every server (upstream `list`).
async fn list(
    servers: &Arc<dyn Fn() -> Vec<Arc<dyn McpResourceServer>> + Send + Sync>,
    params: &Value,
    signal: Option<Arc<AbortSignal>>,
    key: &str,
    page: impl Fn(
        Arc<dyn McpResourceServer>,
        Option<String>,
        McpRequestOptions,
    ) -> BoxFuture<'static, Result<PageList, String>>,
    all: impl Fn(
        Arc<dyn McpResourceServer>,
        McpRequestOptions,
    ) -> BoxFuture<'static, Result<Vec<Value>, String>>,
) -> Result<Value, String> {
    let server_name = string_argument(params, "server")?;
    let cursor = string_argument(params, "cursor")?;
    if let Some(server_name) = server_name {
        let server = find_server(servers, &server_name)?;
        let forwarder = SignalForwarder::new(signal.clone());
        let request = McpRequestOptions {
            timeout_ms: Some(server.timeout_ms()),
            signal: Some(forwarder.token()),
            ..McpRequestOptions::default()
        };
        let result = page(Arc::clone(&server), cursor, request).await?;
        drop(forwarder);
        let (items, next_cursor) = match result {
            PageList::Resources(result) => {
                let items = result
                    .resources
                    .iter()
                    .map(resource_value)
                    .filter(|item| !is_mcp_app_resource(item))
                    .map(|item| listed(server.name(), &item))
                    .collect::<Vec<_>>();
                (items, result.next_cursor)
            }
            PageList::Templates(result) => {
                let items = result
                    .resource_templates
                    .iter()
                    .map(template_value)
                    .filter(|item| !is_mcp_app_resource(item))
                    .map(|item| listed(server.name(), &item))
                    .collect::<Vec<_>>();
                (items, result.next_cursor)
            }
        };
        let mut payload = Map::new();
        payload.insert("server".into(), Value::from(server.name().to_string()));
        payload.insert(key.to_string(), Value::Array(items));
        if let Some(next_cursor) = next_cursor {
            payload.insert("nextCursor".into(), Value::from(next_cursor));
        }
        return Ok(Value::Object(payload));
    }
    if cursor.is_some() {
        return Err("cursor can only be used when a server is specified".to_string());
    }
    let mut sorted = servers();
    sorted.sort_by(|a, b| locale_compare(a.name(), b.name()));
    let mut items: Vec<Value> = Vec::new();
    let mut errors: Vec<Value> = Vec::new();
    for server in sorted {
        let forwarder = SignalForwarder::new(signal.clone());
        let request = McpRequestOptions {
            timeout_ms: Some(server.timeout_ms()),
            signal: Some(forwarder.token()),
            ..McpRequestOptions::default()
        };
        let listed_result = all(Arc::clone(&server), request).await;
        drop(forwarder);
        match listed_result {
            Ok(values) => {
                for value in values {
                    if !is_mcp_app_resource(&value) {
                        items.push(listed(server.name(), &value));
                    }
                }
            }
            Err(error) => errors.push(json!({
                "server": server.name(),
                "error": error,
            })),
        }
    }
    let mut payload = Map::new();
    payload.insert(key.to_string(), Value::Array(items));
    if !errors.is_empty() {
        payload.insert("errors".into(), Value::Array(errors));
    }
    Ok(Value::Object(payload))
}

/// `JSON.stringify` prints integral numbers without a decimal point; the
/// parsed structs carry numbers like `Resource.size` as `f64`, so an
/// integral `3.0` is renormalized to `3`. Values at or beyond `2^53` (where
/// integer precision ends) are left untouched.
fn js_number_shape(value: &mut Value) {
    match value {
        Value::Number(number) => {
            if let Some(as_float) = number.as_f64() {
                if as_float.fract() == 0.0 && as_float.abs() < 9_007_199_254_740_992.0 {
                    *value = Value::Number(serde_json::Number::from(as_float as i64));
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                js_number_shape(item);
            }
        }
        Value::Object(map) => {
            for item in map.values_mut() {
                js_number_shape(item);
            }
        }
        Value::Null | Value::Bool(_) | Value::String(_) => {}
    }
}

/// Wire-order-preserving JSON of a listed resource (struct order + extras).
fn resource_value(resource: &Resource) -> Value {
    let mut value = serde_json::to_value(resource).unwrap_or_else(|_| json!({}));
    js_number_shape(&mut value);
    value
}

fn template_value(template: &ResourceTemplate) -> Value {
    let mut value = serde_json::to_value(template).unwrap_or_else(|_| json!({}));
    js_number_shape(&mut value);
    value
}

/// The three resource tools. `servers` returns the servers whose resources
/// they reach, at call time (upstream `createMcpResourceToolDefinitions`).
pub fn create_mcp_resource_tool_definitions(
    options: McpResourceToolOptions,
) -> Vec<ToolDefinition> {
    let read_only = ToolAnnotations {
        read_only_hint: Some(true),
        ..ToolAnnotations::default()
    };
    let servers: Arc<dyn Fn() -> Vec<Arc<dyn McpResourceServer>> + Send + Sync> =
        Arc::clone(&options.servers);

    let mut list_resources = ToolDefinition::new(
        LIST_MCP_RESOURCES_TOOL,
        LIST_MCP_RESOURCES_TOOL,
        "Lists resources provided by MCP servers. Resources allow servers to share data that provides context to language models, such as files, database schemas, or application-specific information. Prefer resources over web search when possible.",
        list_parameters(),
    );
    list_resources.output_schema = Some(list_output_schema());
    list_resources.exposure = to_tool_exposure(options.exposure);
    list_resources.annotations = Some(read_only.clone());
    list_resources.execute_async = Some(Arc::new(
        move |_tool_call_id: String,
              params: Value,
              signal: Option<Arc<AbortSignal>>,
              _on_update: Option<
            crate::coding_agent::extensions::types::AgentToolUpdateCallbackValue,
        >,
              _ctx: crate::coding_agent::extensions::types::ExtensionContext|
              -> BoxFuture<'static, Result<AgentToolResultValue, String>> {
            let servers = Arc::clone(&servers);
            Box::pin(async move {
                let server_name = string_argument(&params, "server")?;
                let payload = list(
                    &servers,
                    &params,
                    signal,
                    "resources",
                    |server: Arc<dyn McpResourceServer>,
                     cursor: Option<String>,
                     request: McpRequestOptions| {
                        Box::pin(async move {
                            server
                                .resources_page(cursor, request)
                                .await
                                .map(PageList::Resources)
                        })
                    },
                    |server, request| {
                        Box::pin(async move {
                            let resources = server.all_resources(request).await?;
                            Ok(resources.iter().map(resource_value).collect::<Vec<_>>())
                        })
                    },
                )
                .await?;
                json_result(LIST_MCP_RESOURCES_TOOL, server_name.as_deref(), payload).await
            })
        },
    ));

    let servers: Arc<dyn Fn() -> Vec<Arc<dyn McpResourceServer>> + Send + Sync> =
        Arc::clone(&options.servers);
    let mut list_templates = ToolDefinition::new(
        LIST_MCP_RESOURCE_TEMPLATES_TOOL,
        LIST_MCP_RESOURCE_TEMPLATES_TOOL,
        "Lists resource templates provided by MCP servers. Parameterized resource templates allow servers to share data that takes parameters and provides context to language models, such as files, database schemas, or application-specific information. Prefer resource templates over web search when possible.",
        list_parameters(),
    );
    list_templates.output_schema = Some(list_templates_output_schema());
    list_templates.exposure = to_tool_exposure(options.exposure);
    list_templates.annotations = Some(read_only.clone());
    list_templates.execute_async = Some(Arc::new(
        move |_tool_call_id: String,
              params: Value,
              signal: Option<Arc<AbortSignal>>,
              _on_update: Option<
            crate::coding_agent::extensions::types::AgentToolUpdateCallbackValue,
        >,
              _ctx: crate::coding_agent::extensions::types::ExtensionContext|
              -> BoxFuture<'static, Result<AgentToolResultValue, String>> {
            let servers = Arc::clone(&servers);
            Box::pin(async move {
                let server_name = string_argument(&params, "server")?;
                let payload = list(
                    &servers,
                    &params,
                    signal,
                    "resourceTemplates",
                    |server, cursor, request| {
                        Box::pin(async move {
                            server
                                .resource_templates_page(cursor, request)
                                .await
                                .map(PageList::Templates)
                        })
                    },
                    |server, request| {
                        Box::pin(async move {
                            let templates = server.all_resource_templates(request).await?;
                            Ok(templates.iter().map(template_value).collect::<Vec<_>>())
                        })
                    },
                )
                .await?;
                json_result(
                    LIST_MCP_RESOURCE_TEMPLATES_TOOL,
                    server_name.as_deref(),
                    payload,
                )
                .await
            })
        },
    ));

    let servers: Arc<dyn Fn() -> Vec<Arc<dyn McpResourceServer>> + Send + Sync> =
        Arc::clone(&options.servers);
    let mut read_resource = ToolDefinition::new(
        READ_MCP_RESOURCE_TOOL,
        READ_MCP_RESOURCE_TOOL,
        "Read a specific resource from an MCP server given the server name and resource URI.",
        read_parameters(),
    );
    read_resource.output_schema = Some(read_output_schema());
    read_resource.exposure = to_tool_exposure(options.exposure);
    read_resource.annotations = Some(read_only);
    read_resource.execute_async = Some(Arc::new(
        move |_tool_call_id: String,
              params: Value,
              signal: Option<Arc<AbortSignal>>,
              _on_update: Option<
            crate::coding_agent::extensions::types::AgentToolUpdateCallbackValue,
        >,
              _ctx: crate::coding_agent::extensions::types::ExtensionContext|
              -> BoxFuture<'static, Result<AgentToolResultValue, String>> {
            let servers = Arc::clone(&servers);
            Box::pin(async move {
                let server_name = string_argument(&params, "server")?
                    .ok_or_else(|| "server must be provided".to_string())?;
                let uri = string_argument(&params, "uri")?
                    .ok_or_else(|| "uri must be provided".to_string())?;
                let server = find_server(&servers, &server_name)?;
                let forwarder = SignalForwarder::new(signal.clone());
                let request = McpRequestOptions {
                    timeout_ms: Some(server.timeout_ms()),
                    signal: Some(forwarder.token()),
                    ..McpRequestOptions::default()
                };
                let result = server.read_resource(&uri, request).await?;
                drop(forwarder);
                // Several contents (for example a directory) are labeled with
                // their URIs.
                let mut blocks: Vec<Value> = Vec::new();
                for contents in &result.contents {
                    let contents_value = Value::Object(contents.clone());
                    if result.contents.len() > 1 {
                        blocks.push(json!({
                            "type": "text",
                            "text": format!(
                                "{}:",
                                contents_value.get("uri").and_then(Value::as_str).unwrap_or_default()
                            ),
                        }));
                    }
                    blocks.push(json!({ "type": "resource", "resource": contents_value }));
                }
                let converted = to_model_content(server.name(), &blocks, &Default::default()).await;
                let content_source = if converted.is_empty() {
                    vec![LlmContent::Text {
                        text: format!("Resource {uri} is empty."),
                    }]
                } else {
                    converted
                };
                let limited = limit_mcp_content(content_source, None).await?;
                let contents: Vec<Value> = result
                    .contents
                    .iter()
                    .map(|contents| {
                        let mut entry = contents.clone();
                        entry.remove("_meta");
                        Value::Object(entry)
                    })
                    .collect();
                let details = McpToolDetails {
                    server: server.name().to_string(),
                    tool: READ_MCP_RESOURCE_TOOL.to_string(),
                    full_output_path: limited.full_output_path,
                };
                let mut out = Map::new();
                out.insert(
                    "content".into(),
                    convert_llm_content_to_json(&limited.content),
                );
                out.insert("details".into(), details.to_json());
                out.insert(
                    "structuredContent".into(),
                    json!({"server": server.name(), "uri": uri, "contents": contents}),
                );
                Ok(Value::Object(out))
            })
        },
    ));

    vec![list_resources, list_templates, read_resource]
}
