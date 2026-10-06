//! Port of upstream `coding-agent/src/extensions/mcp/tools.ts` (HEAD
//! `2bbfcca43`): adapts MCP tools to pi tool definitions.
//!
//! Results map onto pi's model-facing content (text and images). Text over
//! 20KB keeps its start and end with the middle cut out, like Codex does, and
//! the full text is saved to a temp file the model can read. Binary resources
//! other than images are saved to temp files too, and resource links name the
//! `read_mcp_resource` tool. Codemode scripts receive the whole
//! `CallToolResult` without `_meta` (`content` blocks as sent by the server,
//! `structuredContent`, `isError`), never truncated: it is the tool's
//! `structuredContent`, and every MCP tool declares a `CallToolResult` output
//! schema. MCP errors (`isError`) are error results for the model, but
//! scripts still resolve to the result.
//!
//! Disclosed seams:
//! - **TUI renderers**: `renderCall` / `renderResult` build pi-tui components
//!   (`Text`, `formatToolCallWithArgs`, `keyHint`); the Rust
//!   [`ToolDefinition`] carries no component factories (module-level seam,
//!   see `extensions::types`), so they are cropped.
//! - **`structuredContent` key order**: upstream keeps the server's wire
//!   order for the script result; the port serializes the parsed
//!   `CallToolResult` (fixed field order, extras after). Servers emitting
//!   `content` / `structuredContent` / `isError` in schema order (the
//!   common case, and the oracle's) are byte-identical.
//! - **temp file names**: `randomBytes` is OS entropy, not pinned.

use std::sync::Arc;

use serde_json::{json, Map, Value};

use crate::agent_core::harness::utils::truncate::format_size;
use crate::coding_agent::extensions::types::{
    AbortSignal, AgentToolResultValue, ToolDefinition, ToolExposure, ToolNamespace,
};
use crate::mcp::protocol::content::{to_llm_content, CallToolResult, LlmContent};
use crate::mcp::protocol::types::Tool as McpTool;
use crate::mcp::McpRequestOptions;

/// Upstream `toToolExposure`: tool exposure of an MCP exposure. `codemode`
/// and `deferred` both leave tools out of the codemode description; they
/// differ only in which tool the MCP extension activates to reach them.
pub fn to_tool_exposure(
    exposure: crate::coding_agent::core::mcp_servers::McpExposure,
) -> ToolExposure {
    use crate::coding_agent::core::mcp_servers::McpExposure;
    match exposure {
        McpExposure::Codemode => ToolExposure::Deferred,
        McpExposure::Deferred => ToolExposure::Deferred,
        McpExposure::Direct => ToolExposure::Direct,
        McpExposure::Hidden => ToolExposure::Hidden,
    }
}

/// Provider tool names are limited to 64 characters of `[A-Za-z0-9_]`.
const MAX_TOOL_NAME_LENGTH: usize = 64;
/// Model-facing text of an MCP result beyond this is cut in the middle.
pub const MCP_OUTPUT_MAX_BYTES: usize = 20 * 1024;
#[allow(dead_code)] // consumed by the cropped TUI renderer
const OUTPUT_PREVIEW_LINES: usize = 5;
/// Tool that reads the resources named by resource links.
pub const READ_MCP_RESOURCE_TOOL: &str = "read_mcp_resource";

/// Upstream `McpToolDetails`.
#[derive(Debug, Clone, PartialEq)]
pub struct McpToolDetails {
    pub server: String,
    pub tool: String,
    /// Temp file with the full text output, when the model-facing text was
    /// truncated.
    pub full_output_path: Option<String>,
}

impl McpToolDetails {
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert("server".into(), Value::from(self.server.clone()));
        map.insert("tool".into(), Value::from(self.tool.clone()));
        if let Some(path) = &self.full_output_path {
            map.insert("fullOutputPath".into(), Value::from(path.clone()));
        }
        Value::Object(map)
    }
}

/// Data an output saver receives (upstream `McpOutputSaver`'s
/// `data: string | Uint8Array`): the combined text of a truncated result is a
/// string, a binary resource's bytes are a `Uint8Array`.
pub enum McpSaveData {
    Text(String),
    Bytes(Vec<u8>),
}

impl McpSaveData {
    /// JS `.length`: UTF-16 code units of a string, byte count of a
    /// `Uint8Array`.
    pub fn js_length(&self) -> usize {
        match self {
            McpSaveData::Text(text) => text.encode_utf16().count(),
            McpSaveData::Bytes(bytes) => bytes.len(),
        }
    }

    fn into_vec(self) -> Vec<u8> {
        match self {
            McpSaveData::Text(text) => text.into_bytes(),
            McpSaveData::Bytes(bytes) => bytes,
        }
    }
}

/// Saves the full text of a truncated result, or a binary resource, and
/// returns the file path. `extension` includes the dot, for example `.txt`
/// (upstream `McpOutputSaver`).
pub type McpOutputSaver = Arc<
    dyn Fn(McpSaveData, &str) -> futures::future::BoxFuture<'static, Result<String, String>>
        + Send
        + Sync,
>;

/// Upstream `saveToTempFile`.
pub fn save_to_temp_file(
    data: McpSaveData,
    extension: &str,
) -> futures::future::BoxFuture<'static, Result<String, String>> {
    let extension = extension.to_string();
    Box::pin(async move {
        let mut bytes = [0u8; 8];
        rand::fill(&mut bytes);
        let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let path = std::env::temp_dir().join(format!("pi-mcp-{hex}{extension}"));
        // Results can carry private data, so only the user may read the file.
        write_private(&path, &data.into_vec()).map_err(|error| error.to_string())?;
        Ok(path.to_string_lossy().into_owned())
    })
}

fn write_private(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(data)
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, data)
    }
}

/// `mcp__<server>__<tool>`, sanitized and shortened with a hash suffix when
/// too long. Like Codex, everything but `[A-Za-z0-9_]` becomes `_` (v1.0.0),
/// so the name is also the identifier codemode scripts call it by. `is_taken`
/// reports names used by a different MCP tool: sanitizing can map two tools
/// to one name (`a-b` and `a_b`), which then get the hash suffix (upstream
/// `createMcpToolName`).
pub fn create_mcp_tool_name(server: &str, tool: &str, is_taken: impl Fn(&str) -> bool) -> String {
    let raw = format!("mcp__{server}__{tool}");
    let name: String = raw
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect();
    if name.len() <= MAX_TOOL_NAME_LENGTH && !is_taken(&name) {
        return name;
    }
    let hash = sha256_hex_prefix(&format!("{server}\0{tool}"), 8);
    format!(
        "{}_{hash}",
        truncate_to_byte_boundary(&name, MAX_TOOL_NAME_LENGTH - hash.len() - 1)
    )
}

fn sha256_hex_prefix(input: &str, length: usize) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(input.as_bytes());
    digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()[..length]
        .to_string()
}

/// `name.slice(0, n)` at a UTF-16 code unit index, like JavaScript. The
/// prefix is ASCII (`mcp__`, sanitized), so slicing at `n` bytes is a code
/// unit slice; a multi-byte character straddling the cut drops its whole
/// character here (JS would keep half a surrogate pair — only reachable with
/// astral characters at the exact boundary, disclosed).
fn truncate_to_byte_boundary(name: &str, mut max: usize) -> String {
    if max >= name.len() {
        return name.to_string();
    }
    while max > 0 && !name.is_char_boundary(max) {
        max -= 1;
    }
    name[..max].to_string()
}

/// Text of text blocks, joined with `\n` (upstream `textOf`).
pub fn text_of(content: &[LlmContent]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            LlmContent::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Output schema of every MCP tool: the `CallToolResult` scripts receive,
/// with the tool's own output schema as `structuredContent`. Codemode detects
/// this shape to render `CallToolResult<T>` declarations (upstream
/// `createMcpResultSchema`).
pub fn create_mcp_result_schema(structured_content_schema: Option<&Value>) -> Value {
    let mut properties = Map::new();
    properties.insert(
        "content".into(),
        json!({ "type": "array", "items": { "type": "object" } }),
    );
    if let Some(schema) = structured_content_schema {
        properties.insert("structuredContent".into(), schema.clone());
    }
    properties.insert("isError".into(), json!({ "type": "boolean" }));
    properties.insert("_meta".into(), json!({ "type": "object" }));
    json!({
        "type": "object",
        "properties": properties,
        "required": ["content"],
    })
}

/// Upstream `MiddleTruncationResult` (core/tools/truncate.ts), the subset the
/// MCP limit reads.
pub struct MiddleTruncation {
    pub content: String,
    pub truncated: bool,
    pub removed_chars: usize,
    pub total_bytes: usize,
    pub total_lines: usize,
}

/// Upstream `splitLinesForCounting`.
fn split_lines_for_counting(content: &str) -> usize {
    if content.is_empty() {
        return 0;
    }
    let mut count = content.split('\n').count();
    if content.ends_with('\n') {
        count -= 1;
    }
    count
}

/// Upstream `truncateMiddle` (core/tools/truncate.ts): keep the first and
/// last halves of `max_bytes` UTF-8 bytes, replacing the middle with
/// `…N chars truncated…`. Byte slicing walks to UTF-8 character starts, and
/// the replacement count is code points of the middle (Node `Array.from`).
pub fn truncate_middle(content: &str, max_bytes: usize) -> MiddleTruncation {
    let buf = content.as_bytes();
    let total_lines = split_lines_for_counting(content);
    if buf.len() <= max_bytes {
        return MiddleTruncation {
            content: content.to_string(),
            truncated: false,
            removed_chars: 0,
            total_bytes: buf.len(),
            total_lines,
        };
    }
    let is_boundary = |index: usize| index >= buf.len() || (buf[index] & 0xc0) != 0x80;
    let mut head_end = max_bytes / 2;
    while head_end > 0 && !is_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = buf.len() - (max_bytes - max_bytes / 2);
    while tail_start < buf.len() && !is_boundary(tail_start) {
        tail_start += 1;
    }
    let head = String::from_utf8_lossy(&buf[..head_end]);
    let tail = String::from_utf8_lossy(&buf[tail_start..]);
    let removed_chars = String::from_utf8_lossy(&buf[head_end..tail_start])
        .chars()
        .count();
    MiddleTruncation {
        content: format!("{head}…{removed_chars} chars truncated…{tail}"),
        truncated: true,
        removed_chars,
        total_bytes: buf.len(),
        total_lines,
    }
}

/// Keep model-facing text within [`MCP_OUTPUT_MAX_BYTES`]. Longer text
/// becomes one text block in Codex's truncation format, followed by the path
/// of the file with the full text; images follow it (upstream
/// `limitMcpContent`).
pub fn limit_mcp_content(
    content: Vec<LlmContent>,
    save_output: Option<McpOutputSaver>,
) -> futures::future::BoxFuture<'static, Result<LimitedContent, String>> {
    Box::pin(async move {
        let combined = text_of(&content);
        let truncation = truncate_middle(&combined, MCP_OUTPUT_MAX_BYTES);
        if !truncation.truncated {
            return Ok(LimitedContent {
                content,
                full_output_path: None,
            });
        }
        let saver = save_output
            .unwrap_or_else(|| Arc::new(|data, extension| save_to_temp_file(data, extension)));
        let where_text = match saver(McpSaveData::Text(combined), ".txt").await {
            Ok(full_output_path) => {
                return Ok(LimitedContent {
                    content: build_truncated(
                        &truncation,
                        &format!("[Full output: {full_output_path} (read it with offset/limit)]"),
                        &content,
                    ),
                    full_output_path: Some(full_output_path),
                });
            }
            Err(error) => format!("[Could not save the full output: {error}]"),
        };
        Ok(LimitedContent {
            content: build_truncated(&truncation, &where_text, &content),
            full_output_path: None,
        })
    })
}

fn build_truncated(
    truncation: &MiddleTruncation,
    where_text: &str,
    original: &[LlmContent],
) -> Vec<LlmContent> {
    let tokens = truncation.total_bytes.div_ceil(4);
    let text = format!(
        "Warning: truncated output (original token count: {tokens})\nTotal output lines: {}\n\n{}\n\n{}",
        truncation.total_lines, truncation.content, where_text
    );
    let mut content = vec![LlmContent::Text { text }];
    content.extend(
        original
            .iter()
            .filter(|block| matches!(block, LlmContent::Image { .. }))
            .cloned(),
    );
    content
}

/// Upstream `LimitedMcpContent`.
pub struct LimitedContent {
    pub content: Vec<LlmContent>,
    pub full_output_path: Option<String>,
}

/// Upstream `ConvertMcpResultOptions`.
#[derive(Default, Clone)]
pub struct ConvertMcpResultOptions {
    /// Saves truncated text and binary resources. Default: a temp file.
    pub save_output: Option<McpOutputSaver>,
    /// Whether the server's resources can be read with `read_mcp_resource`,
    /// which resource links then name.
    pub readable_resources: bool,
}

/// File extension for a saved binary resource: the one its URI ends in, else
/// `.bin` (upstream `extensionOf`: `/\.[A-Za-z0-9]{1,8}$/` over the URL path).
fn extension_of(uri: &str) -> String {
    let path = match url::Url::parse(uri) {
        Ok(url) => url.path().to_string(),
        Err(_) => uri.to_string(),
    };
    match path.rfind('.') {
        Some(dot) => {
            let tail = &path[dot..];
            let tail_len = tail.len() - 1;
            if (1..=8).contains(&tail_len)
                && tail[1..].bytes().all(|byte| byte.is_ascii_alphanumeric())
            {
                tail.to_string()
            } else {
                ".bin".to_string()
            }
        }
        None => ".bin".to_string(),
    }
}

/// Blobs of these types are shown as text (upstream `isTextMimeType`).
fn is_text_mime_type(mime_type: Option<&str>) -> bool {
    let Some(mime_type) = mime_type else {
        return false;
    };
    let type_part = mime_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    type_part.starts_with("text/")
        || type_part == "application/json"
        || type_part.ends_with("+json")
        || type_part.ends_with("+xml")
}

/// Model-facing content of one block of `server`'s result (upstream
/// `blockToContent`).
fn block_to_content(
    server: &str,
    block: &Value,
    options: &ConvertMcpResultOptions,
) -> futures::future::BoxFuture<'static, Vec<LlmContent>> {
    let server = server.to_string();
    let block = block.clone();
    let options = options.clone();
    Box::pin(async move {
        let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
        if block_type == "resource_link" {
            let mime_type = block.get("mimeType").and_then(Value::as_str);
            let size = block.get("size").and_then(Value::as_f64);
            let mut details: Vec<String> = Vec::new();
            if let Some(mime_type) = mime_type {
                details.push(mime_type.to_string());
            }
            if let Some(size) = size {
                details.push(format_size(js_number_to_u64(size)));
            }
            let read = if options.readable_resources {
                format!(". Read it with {READ_MCP_RESOURCE_TOOL} (server \"{server}\")")
            } else {
                String::new()
            };
            let description = match block.get("description").and_then(Value::as_str) {
                Some(description) => format!(": {description}"),
                None => String::new(),
            };
            let title = block
                .get("title")
                .and_then(Value::as_str)
                .or_else(|| block.get("name").and_then(Value::as_str))
                .unwrap_or_default();
            let details_text = if details.is_empty() {
                String::new()
            } else {
                format!(" ({})", details.join(", "))
            };
            return vec![LlmContent::Text {
                text: format!(
                    "[Resource {} \"{title}\"{details_text}{description}{read}]",
                    block.get("uri").and_then(Value::as_str).unwrap_or_default()
                ),
            }];
        }
        if block_type == "resource" {
            if let Some(resource) = block.get("resource") {
                let blob = resource.get("blob").and_then(Value::as_str);
                let mime_type = resource.get("mimeType").and_then(Value::as_str);
                let is_image = mime_type.is_some_and(|mime| mime.starts_with("image/"));
                if let Some(blob) = blob {
                    if !is_image {
                        let uri = resource
                            .get("uri")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let data = crate::tui::terminal_image::base64::decode(blob);
                        if is_text_mime_type(mime_type) {
                            return vec![LlmContent::Text {
                                text: String::from_utf8_lossy(&data).into_owned(),
                            }];
                        }
                        let kind = format!(
                            "{}, {}",
                            mime_type.unwrap_or("unknown type"),
                            format_size(data.len() as u64)
                        );
                        let saver = options.save_output.clone().unwrap_or_else(|| {
                            Arc::new(|data, extension| save_to_temp_file(data, extension))
                        });
                        let path = saver(McpSaveData::Bytes(data), &extension_of(uri)).await;
                        return vec![match path {
                            Ok(path) => LlmContent::Text {
                                text: format!("[Binary resource {uri} ({kind}) saved to {path}]"),
                            },
                            Err(reason) => LlmContent::Text {
                                text: format!(
                                    "[Binary resource {uri} ({kind}) could not be saved: {reason}]"
                                ),
                            },
                        }];
                    }
                }
            }
        }
        // Everything else through the shared converter (text, images, audio,
        // embedded text/image resources, unknown types).
        let result = CallToolResult {
            content: vec![block],
            structured_content: None,
            is_error: None,
            meta: None,
            extra: Map::new(),
        };
        to_llm_content(&result)
    })
}

/// `formatSize(size)` of a possibly fractional JSON number.
fn js_number_to_u64(value: f64) -> u64 {
    if value < 0.0 {
        0
    } else {
        value as u64
    }
}

/// Model-facing content of `server`'s content blocks, before the output limit
/// (upstream `toModelContent`).
pub fn to_model_content(
    server: &str,
    blocks: &[Value],
    options: &ConvertMcpResultOptions,
) -> futures::future::BoxFuture<'static, Vec<LlmContent>> {
    let futures: Vec<_> = blocks
        .iter()
        .map(|block| block_to_content(server, block, options))
        .collect();
    Box::pin(async move {
        let mut content = Vec::new();
        for block in futures {
            content.extend(block.await);
        }
        content
    })
}

/// Convert an MCP result. `isError` results become error results that keep
/// the structured result (upstream `convertMcpResult`).
pub fn convert_mcp_result(
    server: &str,
    tool: &str,
    result: &CallToolResult,
    options: &ConvertMcpResultOptions,
) -> futures::future::BoxFuture<'static, Result<AgentToolResultValue, String>> {
    let server = server.to_string();
    let tool = tool.to_string();
    // Serialize without `_meta` (upstream destructures it away): the script
    // result keeps `content`, `structuredContent`, `isError` and any extra
    // fields the server sent.
    let mut script_result = result.clone();
    script_result.meta = None;
    let script_result_value = serde_json::to_value(&script_result).unwrap_or_else(|_| json!({}));
    let is_error = result.is_error.unwrap_or(false);
    let content_blocks = result.content.clone();
    let options = options.clone();
    Box::pin(async move {
        // Without content blocks, toLlmContent falls back to the structured
        // content as JSON.
        let mut converted: Vec<LlmContent> = if !content_blocks.is_empty() {
            to_model_content(&server, &content_blocks, &options).await
        } else {
            to_llm_content(&script_result)
        };
        if is_error && text_of(&converted).is_empty() {
            converted.push(LlmContent::Text {
                text: format!("MCP tool {server}/{tool} returned an error"),
            });
        }
        let limited = limit_mcp_content(converted, options.save_output.clone()).await?;
        let details = McpToolDetails {
            server: server.clone(),
            tool: tool.clone(),
            full_output_path: limited.full_output_path,
        };
        let mut result_map = Map::new();
        result_map.insert("content".into(), llm_content_to_json(&limited.content));
        result_map.insert("details".into(), details.to_json());
        result_map.insert("structuredContent".into(), script_result_value);
        if is_error {
            result_map.insert("isError".into(), Value::Bool(true));
        }
        Ok(Value::Object(result_map))
    })
}

fn llm_content_to_json(content: &[LlmContent]) -> Value {
    Value::Array(
        content
            .iter()
            .map(|block| serde_json::to_value(block).expect("content serialization cannot fail"))
            .collect(),
    )
}

/// [`llm_content_to_json`] for the resource tools.
pub(crate) fn convert_llm_content_to_json(content: &[LlmContent]) -> Value {
    llm_content_to_json(content)
}

/// Tool input schemas must be objects. MCP servers may omit `type`, and some
/// providers reject object schemas without `properties` (upstream
/// `toParameters`).
fn to_parameters(schema: &Map<String, Value>) -> Value {
    let mut out = schema.clone();
    if !out.contains_key("type") {
        out.insert("type".into(), Value::from("object"));
    }
    if !out.contains_key("properties") {
        out.insert("properties".into(), json!({}));
    }
    Value::Object(out)
}

/// The boolean hints of an MCP tool annotation set (upstream
/// `ANNOTATION_HINTS`; the port's `toToolAnnotations` reads them through the
/// typed struct).
#[allow(dead_code)]
const ANNOTATION_HINTS: [&str; 4] = [
    "readOnlyHint",
    "destructiveHint",
    "idempotentHint",
    "openWorldHint",
];

/// The boolean hints of an MCP tool's annotations, or undefined when it has
/// none (upstream `toToolAnnotations`).
fn to_tool_annotations(
    tool: &McpTool,
) -> Option<crate::coding_agent::extensions::types::ToolAnnotations> {
    let mut annotations = crate::coding_agent::extensions::types::ToolAnnotations::default();
    let raw = serde_json::to_value(tool.annotations.as_ref()?).ok()?;
    let mut count = 0;
    for (hint, slot) in [
        ("readOnlyHint", &mut annotations.read_only_hint),
        ("destructiveHint", &mut annotations.destructive_hint),
        ("idempotentHint", &mut annotations.idempotent_hint),
        ("openWorldHint", &mut annotations.open_world_hint),
    ] {
        if let Some(value) = raw.get(hint).and_then(Value::as_bool) {
            *slot = Some(value);
            count += 1;
        }
    }
    (count > 0).then_some(annotations)
}

/// Upstream `McpToolCaller`: the connection surface a tool executes against.
pub trait McpToolCaller: Send + Sync {
    fn call_tool(
        &self,
        name: &str,
        args: Map<String, Value>,
        options: McpRequestOptions,
    ) -> futures::future::BoxFuture<'static, Result<CallToolResult, String>>;
}

/// Upstream `createMcpToolDefinition` options.
///
/// The `getClient` thunk of a registered MCP tool.
pub type GetClientHook = Arc<
    dyn Fn() -> futures::future::BoxFuture<'static, Result<Arc<dyn McpToolCaller>, String>>
        + Send
        + Sync,
>;

pub struct McpToolDefinitionOptions {
    pub server: String,
    pub tool: McpTool,
    pub name: String,
    pub exposure: crate::coding_agent::core::mcp_servers::McpExposure,
    pub namespace: ToolNamespace,
    pub timeout_ms: u64,
    pub get_client: GetClientHook,
    /// Whether `read_mcp_resource` can read the server's resources.
    pub readable_resources: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

/// Build the pi [`ToolDefinition`] of one MCP tool (upstream
/// `createMcpToolDefinition`; the TUI `renderCall`/`renderResult` are cropped,
/// see the module docs).
pub fn create_mcp_tool_definition(options: McpToolDefinitionOptions) -> ToolDefinition {
    let McpToolDefinitionOptions {
        server,
        tool,
        name,
        exposure,
        namespace,
        timeout_ms,
        get_client,
        readable_resources,
    } = options;
    let title = tool
        .title
        .clone()
        .or_else(|| tool.annotations.as_ref().and_then(|a| a.title.clone()));
    let annotations = to_tool_annotations(&tool);
    let label = format!("{server}/{}", tool.name);
    let description = {
        let trimmed = tool.description.as_deref().map(str::trim).unwrap_or("");
        if !trimmed.is_empty() {
            trimmed.to_string()
        } else if let Some(title) = &title {
            title.clone()
        } else {
            format!("MCP tool {} from server {}", tool.name, server)
        }
    };
    let output_schema = tool.output_schema.as_ref();
    let tool_for_execute = tool.clone();

    let mut definition = ToolDefinition::new(
        &name,
        &label,
        &description,
        to_parameters(&tool.input_schema),
    );
    definition.output_schema = Some(create_mcp_result_schema(
        output_schema
            .map(|schema| Value::Object(schema.clone()))
            .as_ref(),
    ));
    definition.exposure = to_tool_exposure(exposure);
    definition.namespace = Some(namespace);
    definition.annotations = annotations;
    definition.execute_async = Some(Arc::new(
        move |_tool_call_id: String,
              params: Value,
              signal: Option<Arc<AbortSignal>>,
              on_update: Option<
            crate::coding_agent::extensions::types::AgentToolUpdateCallbackValue,
        >,
              _ctx: crate::coding_agent::extensions::types::ExtensionContext|
              -> futures::future::BoxFuture<'static, Result<AgentToolResultValue, String>> {
            let tool = tool_for_execute.clone();
            let server = server.clone();
            let get_client = Arc::clone(&get_client);
            let readable = readable_resources
                .as_ref()
                .map(|read| read())
                .unwrap_or(false);
            Box::pin(async move {
                let client = get_client().await?;
                // Forward the extension `AbortSignal` onto a cancellation
                // token for the request (upstream passes the signal through).
                let forward = SignalForwarder::new(signal);
                let mut request = McpRequestOptions {
                    timeout_ms: Some(timeout_ms),
                    signal: Some(forward.token()),
                    ..McpRequestOptions::default()
                };
                if let Some(on_update) = on_update {
                    let server = server.clone();
                    let tool_name = tool.name.clone();
                    request.on_progress = Some(Arc::new(move |progress: &Value| {
                        let total = match progress.get("total").and_then(Value::as_f64) {
                            Some(total) => format!("/{}", js_number(total)),
                            None => String::new(),
                        };
                        let text = match progress.get("message").and_then(Value::as_str) {
                            Some(message) => message.to_string(),
                            None => format!(
                                "Progress {}{}",
                                js_number(
                                    progress
                                        .get("progress")
                                        .and_then(Value::as_f64)
                                        .unwrap_or(0.0)
                                ),
                                total
                            ),
                        };
                        on_update(&json!({
                            "content": [{ "type": "text", "text": text }],
                            "details": { "server": server, "tool": tool_name },
                        }));
                    }));
                }
                let args = match params {
                    // Upstream passes `(params ?? {})` straight through as the
                    // tool-call arguments; schema validation guarantees an
                    // object, so any other shape normalizes to empty here.
                    Value::Object(map) => map,
                    _ => Map::new(),
                };
                let result = client
                    .call_tool(&tool.name, args, request)
                    .await
                    .map_err(|error| error.to_string())?;
                convert_mcp_result(
                    &server,
                    &tool.name,
                    &result,
                    &ConvertMcpResultOptions {
                        readable_resources: readable,
                        ..ConvertMcpResultOptions::default()
                    },
                )
                .await
            })
        },
    ));
    definition
}

/// JS `String(number)`: integers without a decimal point.
pub(crate) fn js_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e21 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// Bridges the extension [`AbortSignal`] onto the `CancellationToken` the MCP
/// request options take; the forwarder task is aborted when the guard drops.
pub(crate) struct SignalForwarder {
    token: tokio_util::sync::CancellationToken,
    _guard: Option<tokio::task::JoinHandle<()>>,
}

impl SignalForwarder {
    pub(crate) fn new(signal: Option<Arc<AbortSignal>>) -> Self {
        let token = tokio_util::sync::CancellationToken::new();
        let mut guard = None;
        if let Some(signal) = signal {
            if signal.is_aborted() {
                token.cancel();
            } else {
                let forward = token.clone();
                guard = Some(tokio::spawn(async move {
                    signal.cancelled().await;
                    forward.cancel();
                }));
            }
        }
        SignalForwarder {
            token,
            _guard: guard,
        }
    }

    pub(crate) fn token(&self) -> tokio_util::sync::CancellationToken {
        self.token.clone()
    }
}

impl Drop for SignalForwarder {
    fn drop(&mut self) {
        if let Some(guard) = self._guard.take() {
            guard.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_are_sanitized_and_hashed() {
        assert_eq!(
            create_mcp_tool_name("docs", "search", |_| false),
            "mcp__docs__search"
        );
        assert_eq!(
            create_mcp_tool_name("my.server", "a.b", |_| false),
            "mcp__my_server__a_b"
        );
        // Collision: second assignment gets the hash suffix.
        assert_eq!(
            create_mcp_tool_name("s", "a.b", |name| name == "mcp__s__a_b"),
            format!("mcp__s__a_b_{}", {
                use sha2::{Digest, Sha256};
                let digest = Sha256::digest(b"s\0a.b");
                digest
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()[..8]
                    .to_string()
            })
        );
        // Over-long names are cut to fit `_<8 hex>`.
        let long_tool = "t".repeat(200);
        let name = create_mcp_tool_name("s", &long_tool, |_| false);
        assert_eq!(name.len(), MAX_TOOL_NAME_LENGTH);
        assert!(name.ends_with(&format!(
            "_{}",
            sha256_hex_prefix(&format!("s\0{long_tool}"), 8)
        )));
    }

    #[test]
    fn result_schema_shape() {
        assert_eq!(
            create_mcp_result_schema(None),
            json!({
                "type": "object",
                "properties": {
                    "content": { "type": "array", "items": { "type": "object" } },
                    "isError": { "type": "boolean" },
                    "_meta": { "type": "object" },
                },
                "required": ["content"],
            })
        );
        let with_structured = create_mcp_result_schema(Some(&json!({ "type": "string" })));
        assert_eq!(
            with_structured["properties"]["structuredContent"],
            json!({ "type": "string" })
        );
    }

    #[tokio::test]
    async fn limits_long_text_with_truncation_marker() {
        let text = "a".repeat(30_000);
        let limited = limit_mcp_content(vec![LlmContent::Text { text }], None)
            .await
            .unwrap();
        assert_eq!(limited.content.len(), 1);
        let output = match &limited.content[0] {
            LlmContent::Text { text } => text.clone(),
            _ => panic!("text"),
        };
        assert!(output.starts_with("Warning: truncated output (original token count: 7500)\n"));
        assert!(output.contains("chars truncated…"));
        assert!(output.contains("(read it with offset/limit)]"));
        assert!(limited.full_output_path.is_some());
        std::fs::remove_file(limited.full_output_path.unwrap()).ok();
    }

    #[test]
    fn truncate_middle_matches_upstream() {
        // Pinned by the oracle's `truncate_middle.ascii` (captured from
        // upstream `truncateMiddle`): the removed count is the code points
        // between the kept head and tail.
        let result = truncate_middle("hello world", 5);
        assert_eq!(result.content, "he…6 chars truncated…rld");
        assert_eq!(result.removed_chars, 6);
        assert_eq!(result.total_bytes, 11);
        assert_eq!(result.total_lines, 1);
        let short = truncate_middle("abc", 5);
        assert!(!short.truncated);
        assert_eq!(short.content, "abc");
    }

    #[test]
    fn text_mime_detection() {
        assert!(is_text_mime_type(Some("text/plain; charset=utf-8")));
        assert!(is_text_mime_type(Some("application/json")));
        assert!(is_text_mime_type(Some("application/vnd.api+json")));
        assert!(is_text_mime_type(Some("application/atom+xml")));
        assert!(!is_text_mime_type(Some("image/png")));
        assert!(!is_text_mime_type(None));
    }

    #[test]
    fn extension_of_uris() {
        assert_eq!(extension_of("file:///a/b.txt"), ".txt");
        assert_eq!(extension_of("https://x/y/blob.PNG"), ".PNG");
        assert_eq!(extension_of("https://x/y/noext"), ".bin");
        assert_eq!(extension_of("not a url"), ".bin");
    }
}
