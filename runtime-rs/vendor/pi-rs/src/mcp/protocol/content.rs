//! Tool result content shapes and the `toLlmContent` converter, ported from
//! upstream `packages/mcp/src/protocol/content.ts`.
//!
//! Content blocks are structural server JSON (upstream types them but only
//! ever reads a known `type` discriminator), so they stay raw
//! [`serde_json::Value`] maps here and [`to_llm_content`] dispatches on the
//! `type` field exactly like the upstream `switch`.

use serde::Serialize;
use serde_json::{Map, Value};

/// Upstream `ContentAnnotations`.
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize, Default)]
pub struct ContentAnnotations {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<f64>,
    #[serde(
        rename = "lastModified",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub last_modified: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Upstream `TextResourceContents` / `BlobResourceContents`.
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct TextResourceContents {
    pub uri: String,
    #[serde(rename = "mimeType", default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    pub text: String,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Map<String, Value>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct BlobResourceContents {
    pub uri: String,
    #[serde(rename = "mimeType", default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    pub blob: String,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Map<String, Value>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Upstream `CallToolResult`. `content` blocks stay raw (servers may emit
/// block types this package does not model; upstream's validation only checks
/// that `content` is an array when present).
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct CallToolResult {
    #[serde(default)]
    pub content: Vec<Value>,
    #[serde(
        rename = "structuredContent",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub structured_content: Option<Map<String, Value>>,
    #[serde(rename = "isError", default, skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Map<String, Value>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Tool result content in the shape LLM APIs accept: text and base64 images.
/// Matches the `TextContent` and `ImageContent` types of
/// `@earendil-works/pi-ai`.
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LlmContent {
    Text {
        text: String,
    },
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
}

impl LlmContent {
    pub fn text(text: impl Into<String>) -> Self {
        LlmContent::Text { text: text.into() }
    }
}

fn block_type(block: &Value) -> &str {
    block
        .as_object()
        .and_then(|object| object.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn block_string<'a>(block: &'a Value, key: &str) -> &'a str {
    block
        .as_object()
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// `String(value)` of a possibly-absent string property: `"undefined"` when
/// the key is missing, exactly like template interpolation in the upstream
/// switch.
fn block_string_or_undefined(block: &Value, key: &str) -> String {
    block
        .as_object()
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| "undefined".to_string())
}

/// Upstream `blockToLlmContent`: text and images pass through; audio,
/// resource links and binary resources become short text placeholders.
fn block_to_llm_content(block: &Value) -> LlmContent {
    match block_type(block) {
        "text" => LlmContent::Text {
            text: block_string(block, "text").to_string(),
        },
        "image" => LlmContent::Image {
            data: block_string(block, "data").to_string(),
            mime_type: block_string(block, "mimeType").to_string(),
        },
        "audio" => LlmContent::Text {
            text: format!(
                "[audio {} omitted]",
                block_string_or_undefined(block, "mimeType")
            ),
        },
        "resource_link" => LlmContent::Text {
            text: format!(
                "{}: {}",
                block_string_or_undefined(block, "name"),
                block_string_or_undefined(block, "uri")
            ),
        },
        "resource" => {
            let resource = block
                .as_object()
                .and_then(|object| object.get("resource"))
                .cloned()
                .unwrap_or(Value::Null);
            if resource
                .as_object()
                .is_some_and(|object| object.contains_key("text"))
            {
                return LlmContent::Text {
                    text: block_string(&resource, "text").to_string(),
                };
            }
            let mime_type = block_string_or_undefined(&resource, "mimeType");
            if mime_type.starts_with("image/") {
                return LlmContent::Image {
                    data: block_string(&resource, "blob").to_string(),
                    mime_type,
                };
            }
            // Upstream `resource.mimeType ?? "unknown type"`.
            let mime_display = if mime_type == "undefined" {
                "unknown type".to_string()
            } else {
                mime_type
            };
            LlmContent::Text {
                text: format!(
                    "[binary resource {} ({}) omitted]",
                    block_string_or_undefined(&resource, "uri"),
                    mime_display,
                ),
            }
        }
        other => LlmContent::Text {
            text: format!("[unsupported MCP content {other}]"),
        },
    }
}

/// Convert a tool result to text and image content for a model. Text and
/// images pass through, embedded text resources become text, embedded image
/// resources become images, and other blocks (audio, resource links, binary
/// resources) become a short text placeholder. A result without content
/// blocks but with `structuredContent` becomes its JSON, since servers
/// should, but do not always, mirror structured results as text.
pub fn to_llm_content(result: &CallToolResult) -> Vec<LlmContent> {
    let mut content: Vec<LlmContent> = result.content.iter().map(block_to_llm_content).collect();
    if content.is_empty() {
        if let Some(structured) = &result.structured_content {
            content.push(LlmContent::Text {
                text: serde_json::to_string_pretty(structured)
                    .expect("Map serialization cannot fail"),
            });
        }
    }
    content
}
