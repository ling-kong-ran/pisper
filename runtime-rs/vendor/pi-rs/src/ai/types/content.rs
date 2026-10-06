//! Content blocks from upstream `packages/ai/src/types.ts:351-387`: the four
//! block shapes a message body can carry (text, thinking, image, tool call) and
//! the `ContentBlock` tagged union over them. Wire format (serde JSON) must
//! match the upstream TypeScript byte-for-byte: the `type` tag uses the
//! upstream literal values (`"toolCall"`, not `"tool_call"`), field names are
//! camelCase (`textSignature`, `thinkingSignature`, `mimeType`,
//! `thoughtSignature`), and optional fields are omitted from JSON when `None`
//! (like upstream `undefined`) so upstream pi session JSONL round-trips.

use serde::{Deserialize, Serialize};

/// Upstream `TextContent` (types.ts:357-361).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextContent {
    pub text: String,
    /// OpenAI Responses message metadata (legacy id string or `TextSignatureV1` JSON).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_signature: Option<String>,
}

/// Upstream `ThinkingContent` (types.ts:363-371).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingContent {
    pub thinking: String,
    /// Provider-specific opaque or serialized reasoning replay data. When
    /// `redacted` is true this holds the opaque encrypted payload instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_signature: Option<String>,
    /// When true, the thinking content was redacted by safety filters; the
    /// encrypted payload rides in `thinkingSignature` for multi-turn continuity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redacted: Option<bool>,
}

/// Upstream `ImageContent` (types.ts:373-377).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageContent {
    /// Base64 encoded image data.
    pub data: String,
    /// e.g. `"image/jpeg"`, `"image/png"` (wire name: `mimeType`).
    pub mime_type: String,
}

/// Upstream `ToolCall` (types.ts:379-387).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Tool arguments (upstream `JsonObject`, types.ts:383). Deliberate
    /// widening: `serde_json::Value` admits non-object JSON where upstream's
    /// record type admits only objects, so struct-level round-trips are a
    /// superset of upstream's. Wire output is unaffected — every
    /// upstream-valid `arguments` serializes byte-identically.
    pub arguments: serde_json::Value,
    /// Google-specific: opaque signature for reusing thought context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
    /// OpenAI Responses namespace for calls to dynamically loaded or namespaced tools.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}

/// One message body block, tagged by its upstream `type` field
/// (types.ts:357-387). Tag values are the upstream literals; camelCase maps
/// `ToolCall` to `"toolCall"` (never `"tool_call"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ContentBlock {
    Text(TextContent),
    Thinking(ThinkingContent),
    Image(ImageContent),
    ToolCall(ToolCall),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_block_tag_values_match_upstream() {
        let cases = [
            (
                ContentBlock::Text(TextContent {
                    text: "hello".into(),
                    text_signature: None,
                }),
                r#"{"type":"text""#,
            ),
            (
                ContentBlock::Thinking(ThinkingContent {
                    thinking: "hmm".into(),
                    thinking_signature: None,
                    redacted: None,
                }),
                r#"{"type":"thinking""#,
            ),
            (
                ContentBlock::Image(ImageContent {
                    data: "aGVsbG8=".into(),
                    mime_type: "image/png".into(),
                }),
                r#"{"type":"image""#,
            ),
            (
                ContentBlock::ToolCall(ToolCall {
                    id: "call_1".into(),
                    name: "bash".into(),
                    arguments: serde_json::json!({}),
                    thought_signature: None,
                    namespace: None,
                }),
                r#"{"type":"toolCall""#,
            ),
        ];
        for (block, tag_prefix) in cases {
            let json = serde_json::to_string(&block).unwrap();
            assert!(
                json.starts_with(tag_prefix),
                "expected {tag_prefix} prefix, got {json}"
            );
            let back: ContentBlock = serde_json::from_str(&json).unwrap();
            assert_eq!(back, block);
        }
        // The toolCall tag must be camelCase, never snake_case.
        let tool_call = serde_json::to_string(&ContentBlock::ToolCall(ToolCall {
            id: "call_1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({}),
            thought_signature: None,
            namespace: None,
        }))
        .unwrap();
        assert!(!tool_call.contains("tool_call"), "{tool_call}");
    }

    #[test]
    fn text_block_round_trips_without_optional_fields() {
        let fixture = r#"{"type":"text","text":"hello world"}"#;
        let block: ContentBlock = serde_json::from_str(fixture).unwrap();
        assert_eq!(
            block,
            ContentBlock::Text(TextContent {
                text: "hello world".into(),
                text_signature: None,
            })
        );
        assert_eq!(serde_json::to_string(&block).unwrap(), fixture);
    }

    #[test]
    fn text_block_round_trips_with_text_signature() {
        let fixture = r#"{"type":"text","text":"hello world","textSignature":"rs_v1:abc123"}"#;
        let block: ContentBlock = serde_json::from_str(fixture).unwrap();
        assert_eq!(
            block,
            ContentBlock::Text(TextContent {
                text: "hello world".into(),
                text_signature: Some("rs_v1:abc123".into()),
            })
        );
        assert_eq!(serde_json::to_string(&block).unwrap(), fixture);
    }

    #[test]
    fn thinking_block_round_trips_without_optional_fields() {
        let fixture = r#"{"type":"thinking","thinking":"let me reason step by step"}"#;
        let block: ContentBlock = serde_json::from_str(fixture).unwrap();
        assert_eq!(
            block,
            ContentBlock::Thinking(ThinkingContent {
                thinking: "let me reason step by step".into(),
                thinking_signature: None,
                redacted: None,
            })
        );
        assert_eq!(serde_json::to_string(&block).unwrap(), fixture);
    }

    #[test]
    fn thinking_block_round_trips_with_signature_and_redacted() {
        let fixture = r#"{"type":"thinking","thinking":"encrypted payload","thinkingSignature":"enc:xyz","redacted":true}"#;
        let block: ContentBlock = serde_json::from_str(fixture).unwrap();
        assert_eq!(
            block,
            ContentBlock::Thinking(ThinkingContent {
                thinking: "encrypted payload".into(),
                thinking_signature: Some("enc:xyz".into()),
                redacted: Some(true),
            })
        );
        assert_eq!(serde_json::to_string(&block).unwrap(), fixture);
        let unredacted: ContentBlock =
            serde_json::from_str(r#"{"type":"thinking","thinking":"hmm","redacted":false}"#)
                .unwrap();
        assert_eq!(
            unredacted,
            ContentBlock::Thinking(ThinkingContent {
                thinking: "hmm".into(),
                thinking_signature: None,
                redacted: Some(false),
            })
        );
    }

    #[test]
    fn image_block_round_trips() {
        let fixture = r#"{"type":"image","data":"aGVsbG8gd29ybGQ=","mimeType":"image/jpeg"}"#;
        let block: ContentBlock = serde_json::from_str(fixture).unwrap();
        assert_eq!(
            block,
            ContentBlock::Image(ImageContent {
                data: "aGVsbG8gd29ybGQ=".into(),
                mime_type: "image/jpeg".into(),
            })
        );
        assert_eq!(serde_json::to_string(&block).unwrap(), fixture);
    }

    #[test]
    fn tool_call_block_round_trips_without_optional_fields() {
        let fixture = r#"{"type":"toolCall","id":"call_1","name":"bash","arguments":{"command":["ls","-la"],"timeout":5000}}"#;
        let block: ContentBlock = serde_json::from_str(fixture).unwrap();
        assert_eq!(
            block,
            ContentBlock::ToolCall(ToolCall {
                id: "call_1".into(),
                name: "bash".into(),
                arguments: serde_json::json!({"command": ["ls", "-la"], "timeout": 5000}),
                thought_signature: None,
                namespace: None,
            })
        );
        assert_eq!(serde_json::to_string(&block).unwrap(), fixture);
    }

    #[test]
    fn tool_call_block_round_trips_with_optional_fields() {
        let fixture = r#"{"type":"toolCall","id":"call_2","name":"edit","arguments":{"path":"a.txt"},"thoughtSignature":"sig_abc","namespace":"browser"}"#;
        let block: ContentBlock = serde_json::from_str(fixture).unwrap();
        assert_eq!(
            block,
            ContentBlock::ToolCall(ToolCall {
                id: "call_2".into(),
                name: "edit".into(),
                arguments: serde_json::json!({"path": "a.txt"}),
                thought_signature: Some("sig_abc".into()),
                namespace: Some("browser".into()),
            })
        );
        assert_eq!(serde_json::to_string(&block).unwrap(), fixture);
    }

    #[test]
    fn tool_call_arguments_object_preserves_json_shape() {
        // serde_json::Value must round-trip the argument object exactly:
        // integers stay integers, nested objects/arrays/null survive intact.
        let arguments: serde_json::Value = serde_json::from_str(
            r#"{"count":42,"ratio":0.5,"flag":true,"note":null,"nested":{"deep":[1,"two"]}}"#,
        )
        .unwrap();
        let block = ContentBlock::ToolCall(ToolCall {
            id: "call_3".into(),
            name: "complex".into(),
            arguments,
            thought_signature: None,
            namespace: None,
        });
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains(r#""count":42,"#), "{json}");
        let back: ContentBlock = serde_json::from_str(&json).unwrap();
        assert_eq!(back, block);
    }

    #[test]
    fn optional_fields_are_skipped_when_none() {
        let json = serde_json::to_string(&ContentBlock::Text(TextContent {
            text: "hi".into(),
            text_signature: None,
        }))
        .unwrap();
        assert!(!json.contains("textSignature"), "{json}");

        let json = serde_json::to_string(&ContentBlock::Thinking(ThinkingContent {
            thinking: "hmm".into(),
            thinking_signature: None,
            redacted: None,
        }))
        .unwrap();
        assert!(!json.contains("thinkingSignature"), "{json}");
        assert!(!json.contains("redacted"), "{json}");

        let json = serde_json::to_string(&ContentBlock::ToolCall(ToolCall {
            id: "call_1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({}),
            thought_signature: None,
            namespace: None,
        }))
        .unwrap();
        assert!(!json.contains("thoughtSignature"), "{json}");
        assert!(!json.contains("namespace"), "{json}");
    }

    #[test]
    fn bare_structs_round_trip_without_tag() {
        // The structs are the tagless payload; ContentBlock owns the `type` tag.
        // Bare structs still round-trip on their own fields.
        let text = TextContent {
            text: "hi".into(),
            text_signature: Some("sig".into()),
        };
        let back: TextContent =
            serde_json::from_str(&serde_json::to_string(&text).unwrap()).unwrap();
        assert_eq!(back, text);

        let call = ToolCall {
            id: "call_1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({"cmd": "ls"}),
            thought_signature: None,
            namespace: None,
        };
        let back: ToolCall = serde_json::from_str(&serde_json::to_string(&call).unwrap()).unwrap();
        assert_eq!(back, call);
    }
}
