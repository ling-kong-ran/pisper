//! Message types from upstream `packages/ai/src/types.ts:462-546`: the four
//! transcript roles (`system`, `user`, `assistant`, `toolResult`), the
//! string-or-blocks content unions, and the assistant diagnostic port from
//! `packages/ai/src/utils/diagnostics.ts`. Wire format (serde JSON) must match
//! the upstream TypeScript byte-for-byte: the `Message` enum is tagged by
//! `role` with the upstream literal values (`"toolResult"`, not
//! `"tool_result"`), field names are camelCase (`toolCallId`, `toolsAdded`,
//! `toolsRemoved`, `isError`, `rawStopReason`, `responseModel`,
//! `providerThinkingLevel`, `endTurn`), optional fields are omitted from JSON
//! when `None` (like upstream `undefined`), and timestamps are Unix
//! milliseconds (`number` upstream, `i64` here) so upstream pi session JSONL
//! round-trips.

use serde::de::{Deserializer, MapAccess, Visitor};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use std::fmt;

use super::content::{ImageContent, TextContent, ThinkingContent, ToolCall};
use super::options::DeferredHandle;
use super::primitives::{StopReason, Usage};
use super::tool::{Tool, ToolReference};

/// Upstream `AssistantMessage.content` element union (types.ts:510):
/// `TextContent | ThinkingContent | ToolCall`, tagged by the blocks' `type`
/// field (camelCase maps `ToolCall` to `"toolCall"`). Images are not valid in
/// assistant content upstream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AssistantBlock {
    Text(TextContent),
    Thinking(ThinkingContent),
    ToolCall(ToolCall),
}

/// Upstream `TextContent | ImageContent` (types.ts:504, 537): the block union
/// allowed in user messages and tool results, tagged by the blocks' `type`
/// field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum TextOrImageBlock {
    Text(TextContent),
    Image(ImageContent),
}

/// Upstream `string | TextContent[]` (`SystemMessage.content`, types.ts:487)
/// and `string | (TextContent | ImageContent)[]` (`UserMessage.content`,
/// types.ts:504): a bare plain-text string or a block array. Untagged so both
/// wire shapes serialize/deserialize exactly as written. The block element type
/// is the wider `TextOrImageBlock` shared by both roles; system messages
/// upstream restrict blocks to `TextContent[]`, but that restriction is
/// compile-time only in TypeScript (no runtime validation) and the wire never
/// carries images there, so one union covers both roles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StringOrBlocks {
    Text(String),
    Blocks(Vec<TextOrImageBlock>),
}

/// Upstream `DiagnosticErrorInfo` (utils/diagnostics.ts:3-8): redacted error
/// details attached to an assistant diagnostic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticErrorInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
    /// Upstream `code?: string | number` (diagnostics.ts:7).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<DiagnosticCode>,
}

/// The inline `string | number` union of `DiagnosticErrorInfo.code`
/// (diagnostics.ts:7). `serde_json::Number` preserves integer vs float.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DiagnosticCode {
    String(String),
    Number(serde_json::Number),
}

/// Upstream `AssistantMessageDiagnostic` (utils/diagnostics.ts:10-15):
/// redacted provider/runtime diagnostic for failures and recoveries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantMessageDiagnostic {
    pub r#type: String,
    pub timestamp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<DiagnosticErrorInfo>,
    /// Upstream `details?: JsonObject`; `serde_json::Value` is a superset
    /// (same pattern as `ToolCall.arguments`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// Upstream `SystemMessage.sections` (types.ts:494, `Record<string, string | null>`):
/// named prompt sections whose order is semantic. Upstream renders sections
/// verbatim after `content` in the object's key insertion order
/// (`Object.values` in utils/text.ts:17) and the transcript replay
/// (utils/transcript.ts:75-92) rebuilds the wire object in first-appearance
/// order — so the container must preserve order, not sort it. A `Vec` of pairs
/// with hand-written serde keeps that order while the wire stays a JSON object
/// exactly like upstream: entries serialize in stored order, and
/// deserialization reads them in document order with JavaScript object
/// semantics for duplicate keys (first position kept, last value wins).
/// Upstream warns against integer-like section names because JS objects
/// reorder those (types.ts:492); this container deliberately does not, so
/// document order survives the round-trip byte-identically.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sections(Vec<(String, Option<String>)>);

impl Sections {
    /// Builds sections from name/value pairs, in render order.
    pub fn new(pairs: Vec<(String, Option<String>)>) -> Self {
        Self(pairs)
    }

    /// All entries in order. `Some` values are live section text; `None` is a
    /// removal marker (only meaningful in patch messages — the transcript
    /// replay never emits `None`).
    pub fn as_slice(&self) -> &[(String, Option<String>)] {
        &self.0
    }

    /// The value stored for `name`, if the section is present.
    pub fn get(&self, name: &str) -> Option<&Option<String>> {
        self.0
            .iter()
            .find(|(existing, _)| existing == name)
            .map(|(_, value)| value)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl<'a> IntoIterator for &'a Sections {
    type Item = &'a (String, Option<String>);
    type IntoIter = std::slice::Iter<'a, (String, Option<String>)>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl Serialize for Sections {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (name, value) in &self.0 {
            map.serialize_entry(name, value)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Sections {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SectionsVisitor;

        impl<'de> Visitor<'de> for SectionsVisitor {
            type Value = Sections;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a map of section names to text or null")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut pairs: Vec<(String, Option<String>)> = Vec::new();
                while let Some((name, value)) = access.next_entry::<String, Option<String>>()? {
                    match pairs.iter_mut().find(|(existing, _)| *existing == name) {
                        // JS object semantics: a duplicate key keeps its first
                        // position but takes the last value.
                        Some(entry) => entry.1 = value,
                        None => pairs.push((name, value)),
                    }
                }
                Ok(Sections(pairs))
            }
        }

        deserializer.deserialize_map(SectionsVisitor)
    }
}

/// Upstream `SystemMessage` (types.ts:484-500): system instructions and tool
/// declarations at one point in the transcript. The leading system message is
/// the system prompt; later messages patch it — `content` adds instructions
/// from that point on, `sections` replaces named prompt sections by name
/// (`null` removes one), and `toolsAdded`/`toolsRemoved` change the tool set.
/// Replaying every system message in order yields the current prompt and
/// tools.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemMessage {
    /// Instruction text. On the leading message this is the base prompt;
    /// later, additional instructions.
    pub content: StringOrBlocks,
    /// Named, ordered prompt sections rendered verbatim after `content`.
    /// Order is semantic (prompt layout); see [`Sections`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sections: Option<Sections>,
    /// Complete definitions of tools that become available at this point.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools_added: Option<Vec<Tool>>,
    /// Tools that stop being available at this point.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools_removed: Option<Vec<ToolReference>>,
    /// Unix timestamp in milliseconds.
    pub timestamp: i64,
}

/// Upstream `UserMessage` (types.ts:502-506).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserMessage {
    pub content: StringOrBlocks,
    /// Unix timestamp in milliseconds.
    pub timestamp: i64,
}

/// Upstream `AssistantMessage` (types.ts:508-530).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantMessage {
    pub content: Vec<AssistantBlock>,
    /// Upstream `Api` (`KnownApi | string`, types.ts:29): open-ended, so a
    /// plain string checked against `KNOWN_API`.
    pub api: String,
    /// Upstream `ProviderId` (`KnownProvider | string`, types.ts:76).
    pub provider: String,
    pub model: String,
    /// Concrete model reported by the provider when different from the
    /// requested `model`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_model: Option<String>,
    /// Provider-specific response/message identifier when the upstream API
    /// exposes one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    /// Exact provider-native effort level used for this response. Absent for
    /// legacy or unmanaged responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_thinking_level: Option<String>,
    /// Redacted provider/runtime diagnostics for failures and recoveries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Vec<AssistantMessageDiagnostic>>,
    pub usage: Usage,
    pub stop_reason: StopReason,
    /// Durable handle for deferred responses: set when a capable provider
    /// continues the request asynchronously (upstream `DeferredHandle`,
    /// types.ts:462-472, defined in `options.rs`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deferred: Option<DeferredHandle>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    /// Provider-native stop reason before pi's normalization.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_stop_reason: Option<String>,
    /// Provider indication of whether the model explicitly ended its turn.
    /// Preserved for debugging; does not affect agent control flow.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_turn: Option<bool>,
    /// Unix timestamp in milliseconds.
    pub timestamp: i64,
}

/// Upstream `ToolResultMessage` (types.ts:532-544) at its default
/// `TDetails = JsonValue`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultMessage {
    pub tool_call_id: String,
    pub tool_name: String,
    /// Tool output; supports text and images.
    pub content: Vec<TextOrImageBlock>,
    /// Structured execution details (upstream `JsonRepresentation<JsonValue>`).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::serde_support::present_json"
    )]
    pub details: Option<serde_json::Value>,
    /// Usage from the tool execution itself, if available. Not part of main
    /// LLM context accounting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    pub is_error: bool,
    /// Unix timestamp in milliseconds.
    pub timestamp: i64,
}

/// Upstream `Message` (types.ts:546): tagged by `role` with the upstream
/// literal values (`"toolResult"`, never `"tool_result"`).
///
/// The Assistant variant is intrinsically the largest payload (content blocks,
/// usage, diagnostics, deferred handle); the size difference against the other
/// variants crossed clippy's default threshold when `deferred` became the
/// typed `DeferredHandle`. Boxing the variant (or the handle) would add
/// indirection at every use site for no functional gain.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "camelCase")]
pub enum Message {
    System(SystemMessage),
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::types::UsageCost;

    const TS: i64 = 1758240000000;

    // serde_json Map keys sort lexicographically (no preserve_order feature),
    // so fixtures keep object keys inside `serde_json::Value` fields sorted.
    const USER_BARE_STRING: &str =
        r#"{"role":"user","content":"hi there","timestamp":1758240000000}"#;

    const USER_BLOCKS: &str = r#"{"role":"user","content":[{"type":"text","text":"what is this?"},{"type":"image","data":"aGVsbG8=","mimeType":"image/png"}],"timestamp":1758240000001}"#;

    const SYSTEM_BARE_STRING: &str =
        r#"{"role":"system","content":"You are pi.","timestamp":1758240000002}"#;

    const SYSTEM_FULL_PATCH: &str = r#"{"role":"system","content":[{"type":"text","text":"Extra instructions from here on."}],"sections":{"tools":"Use tools carefully.","skills":null},"toolsAdded":[{"name":"bash","description":"Run a shell command","parameters":{"properties":{"command":{"type":"string"}},"type":"object"}}],"toolsRemoved":[{"name":"weather"}],"timestamp":1758240000003}"#;

    const ASSISTANT_FULL: &str = r#"{"role":"assistant","content":[{"type":"text","text":"Running ls."},{"type":"thinking","thinking":"need the listing","thinkingSignature":"sig1"},{"type":"toolCall","id":"call_1","name":"bash","arguments":{"command":"ls"}}],"api":"anthropic-messages","provider":"anthropic","model":"claude-sonnet-4-5","responseModel":"claude-sonnet-4-5-20250929","responseId":"msg_01ABC","providerThinkingLevel":"high","diagnostics":[{"type":"retry","timestamp":1758240000000,"error":{"name":"HttpError","message":"429 too many requests","stack":"at f()","code":429},"details":{"attempt":1}}],"usage":{"input":10,"output":5,"cacheRead":0,"cacheWrite":0,"cacheWrite1h":0,"reasoning":5,"totalTokens":15,"cost":{"input":0.01,"output":0.02,"cacheRead":0,"cacheWrite":0,"total":0.03}},"stopReason":"toolUse","deferred":{"provider":"anthropic","modelId":"claude-sonnet-4-5","api":"anthropic-messages","id":"resp_123"},"errorMessage":"first attempt failed","rawStopReason":"tool_use","endTurn":false,"timestamp":1758240000004}"#;

    const ASSISTANT_MINIMAL: &str = r#"{"role":"assistant","content":[],"api":"openai-completions","provider":"openai","model":"gpt-5","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"stopReason":"stop","timestamp":1758240000005}"#;

    const TOOL_RESULT_FULL: &str = r#"{"role":"toolResult","toolCallId":"call_1","toolName":"bash","content":[{"type":"text","text":"total 0"},{"type":"image","data":"aGVsbG8=","mimeType":"image/png"}],"details":{"exitCode":0,"files":["a.txt"]},"usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"isError":false,"timestamp":1758240000006}"#;

    const TOOL_RESULT_MINIMAL_ERROR: &str = r#"{"role":"toolResult","toolCallId":"call_2","toolName":"edit","content":[{"type":"text","text":"file not found"}],"isError":true,"timestamp":1758240000007}"#;

    #[test]
    fn user_bare_string_round_trips_via_message_and_struct() {
        let msg: Message = serde_json::from_str(USER_BARE_STRING).unwrap();
        let Message::User(user) = &msg else {
            panic!("expected user variant, got {msg:?}");
        };
        assert_eq!(user.content, StringOrBlocks::Text("hi there".into()));
        assert_eq!(user.timestamp, 1758240000000);
        assert_eq!(serde_json::to_string(&msg).unwrap(), USER_BARE_STRING);

        let bare: UserMessage = serde_json::from_str(r#"{"content":"hi","timestamp":1}"#).unwrap();
        assert_eq!(bare.content, StringOrBlocks::Text("hi".into()));
        assert_eq!(
            serde_json::to_string(&bare).unwrap(),
            r#"{"content":"hi","timestamp":1}"#
        );
    }

    #[test]
    fn user_block_array_round_trips() {
        let msg: Message = serde_json::from_str(USER_BLOCKS).unwrap();
        let Message::User(user) = &msg else {
            panic!("expected user variant, got {msg:?}");
        };
        let StringOrBlocks::Blocks(blocks) = &user.content else {
            panic!("expected blocks, got {:?}", user.content);
        };
        assert_eq!(blocks.len(), 2);
        assert_eq!(
            blocks[0],
            TextOrImageBlock::Text(TextContent {
                text: "what is this?".into(),
                text_signature: None,
            })
        );
        assert_eq!(
            blocks[1],
            TextOrImageBlock::Image(ImageContent {
                data: "aGVsbG8=".into(),
                mime_type: "image/png".into(),
            })
        );
        assert_eq!(serde_json::to_string(&msg).unwrap(), USER_BLOCKS);
    }

    #[test]
    fn system_bare_string_round_trips_via_message_and_struct() {
        let msg: Message = serde_json::from_str(SYSTEM_BARE_STRING).unwrap();
        let Message::System(system) = &msg else {
            panic!("expected system variant, got {msg:?}");
        };
        assert_eq!(system.content, StringOrBlocks::Text("You are pi.".into()));
        assert_eq!(system.sections, None);
        assert_eq!(system.tools_added, None);
        assert_eq!(system.tools_removed, None);
        assert_eq!(serde_json::to_string(&msg).unwrap(), SYSTEM_BARE_STRING);

        let bare: SystemMessage =
            serde_json::from_str(r#"{"content":"base prompt","timestamp":1}"#).unwrap();
        assert_eq!(bare.content, StringOrBlocks::Text("base prompt".into()));
        assert_eq!(
            serde_json::to_string(&bare).unwrap(),
            r#"{"content":"base prompt","timestamp":1}"#
        );
    }

    #[test]
    fn system_full_patch_round_trips() {
        let msg: Message = serde_json::from_str(SYSTEM_FULL_PATCH).unwrap();
        let Message::System(system) = &msg else {
            panic!("expected system variant, got {msg:?}");
        };
        let StringOrBlocks::Blocks(blocks) = &system.content else {
            panic!("expected blocks, got {:?}", system.content);
        };
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            blocks[0],
            TextOrImageBlock::Text(TextContent {
                text: "Extra instructions from here on.".into(),
                text_signature: None,
            })
        );
        let sections = system.sections.as_ref().unwrap();
        // Document order ("tools" before "skills") is preserved, not sorted
        // lexicographically.
        let names: Vec<&str> = sections
            .as_slice()
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(names, ["tools", "skills"]);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections.get("skills"), Some(&None));
        assert_eq!(
            sections.get("tools"),
            Some(&Some("Use tools carefully.".into()))
        );
        let added = system.tools_added.as_ref().unwrap();
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].name, "bash");
        assert_eq!(added[0].description, "Run a shell command");
        assert_eq!(
            added[0].parameters,
            serde_json::json!({"properties":{"command":{"type":"string"}},"type":"object"})
        );
        let removed = system.tools_removed.as_ref().unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].name, "weather");
        assert_eq!(serde_json::to_string(&msg).unwrap(), SYSTEM_FULL_PATCH);
    }

    #[test]
    fn sections_order_survives_message_deserialization_and_round_trips() {
        // Non-lexicographic order through the internally-tagged `Message`
        // enum: serde's Content buffering must not reorder the object keys.
        let wire = r#"{"role":"system","content":"base","sections":{"zeta":"last","alpha":"first","mid":null},"timestamp":1}"#;
        let msg: Message = serde_json::from_str(wire).unwrap();
        let Message::System(system) = &msg else {
            panic!("expected system variant, got {msg:?}");
        };
        let sections = system.sections.as_ref().unwrap();
        let names: Vec<&str> = sections
            .as_slice()
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(names, ["zeta", "alpha", "mid"]);
        // Serialization writes back in stored order, so the round-trip is
        // byte-identical even when the order is not sorted.
        assert_eq!(serde_json::to_string(&msg).unwrap(), wire);
    }

    #[test]
    fn sections_duplicate_keys_keep_first_position_take_last_value() {
        // JavaScript object semantics for duplicate JSON keys: JSON.parse
        // keeps the first insertion position but the last value wins.
        let wire = r#"{"name":"z","name":"a"}"#;
        let sections: Sections = serde_json::from_str(wire).unwrap();
        let names: Vec<&str> = sections
            .as_slice()
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(names, ["name"]);
        assert_eq!(sections.get("name"), Some(&Some("a".into())));
        // Serializing writes the surviving single entry.
        assert_eq!(serde_json::to_string(&sections).unwrap(), r#"{"name":"a"}"#);
    }

    #[test]
    fn sections_null_values_round_trip_and_bare_struct_helpers_work() {
        let sections = Sections::new(vec![
            ("rules".into(), Some("<rules>x</rules>".into())),
            ("skills".into(), None),
        ]);
        assert_eq!(
            serde_json::to_string(&sections).unwrap(),
            r#"{"rules":"<rules>x</rules>","skills":null}"#
        );
        let back: Sections =
            serde_json::from_str(r#"{"rules":"<rules>x</rules>","skills":null}"#).unwrap();
        assert_eq!(back, sections);
        assert!(!sections.is_empty());
        assert_eq!(sections.len(), 2);
        assert!(Sections::default().is_empty());
        assert_eq!(Sections::default().get("rules"), None);
    }

    #[test]
    fn system_message_omits_none_optional_fields() {
        let system = SystemMessage {
            content: StringOrBlocks::Text("base".into()),
            sections: None,
            tools_added: None,
            tools_removed: None,
            timestamp: TS,
        };
        let json = serde_json::to_string(&system).unwrap();
        for key in ["sections", "toolsAdded", "toolsRemoved"] {
            assert!(!json.contains(key), "{key} leaked: {json}");
        }
    }

    #[test]
    fn assistant_message_full_round_trips() {
        let msg: Message = serde_json::from_str(ASSISTANT_FULL).unwrap();
        let Message::Assistant(assistant) = &msg else {
            panic!("expected assistant variant, got {msg:?}");
        };
        assert_eq!(assistant.content.len(), 3);
        assert_eq!(
            assistant.content[0],
            AssistantBlock::Text(TextContent {
                text: "Running ls.".into(),
                text_signature: None,
            })
        );
        assert!(matches!(&assistant.content[1], AssistantBlock::Thinking(t)
            if t.thinking == "need the listing" && t.thinking_signature == Some("sig1".into())));
        assert!(
            matches!(&assistant.content[2], AssistantBlock::ToolCall(call)
            if call.id == "call_1" && call.name == "bash")
        );
        assert_eq!(assistant.api, "anthropic-messages");
        assert_eq!(assistant.provider, "anthropic");
        assert_eq!(assistant.model, "claude-sonnet-4-5");
        assert_eq!(
            assistant.response_model,
            Some("claude-sonnet-4-5-20250929".into())
        );
        assert_eq!(assistant.response_id, Some("msg_01ABC".into()));
        assert_eq!(assistant.provider_thinking_level, Some("high".into()));
        let diagnostics = assistant.diagnostics.as_ref().unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].r#type, "retry");
        assert_eq!(diagnostics[0].timestamp, 1758240000000);
        let error = diagnostics[0].error.as_ref().unwrap();
        assert_eq!(error.name, Some("HttpError".into()));
        assert_eq!(error.message, "429 too many requests");
        assert_eq!(error.stack, Some("at f()".into()));
        assert_eq!(
            error.code,
            Some(DiagnosticCode::Number(serde_json::Number::from(429)))
        );
        assert_eq!(
            diagnostics[0].details,
            Some(serde_json::json!({"attempt": 1}))
        );
        assert_eq!(assistant.usage.input, 10);
        assert_eq!(assistant.usage.reasoning, Some(5));
        assert_eq!(assistant.stop_reason, StopReason::ToolUse);
        assert_eq!(
            assistant.deferred,
            Some(DeferredHandle {
                provider: "anthropic".into(),
                model_id: "claude-sonnet-4-5".into(),
                api: "anthropic-messages".into(),
                id: "resp_123".into(),
                expires_at: None,
                poll_after_ms: None,
                data: None,
            })
        );
        assert_eq!(assistant.error_message, Some("first attempt failed".into()));
        assert_eq!(assistant.raw_stop_reason, Some("tool_use".into()));
        assert_eq!(assistant.end_turn, Some(false));
        assert_eq!(serde_json::to_string(&msg).unwrap(), ASSISTANT_FULL);
    }

    #[test]
    fn assistant_message_constructed_with_all_optionals_matches_fixture_bytes() {
        let assistant = AssistantMessage {
            content: vec![
                AssistantBlock::Text(TextContent {
                    text: "Running ls.".into(),
                    text_signature: None,
                }),
                AssistantBlock::Thinking(ThinkingContent {
                    thinking: "need the listing".into(),
                    thinking_signature: Some("sig1".into()),
                    redacted: None,
                }),
                AssistantBlock::ToolCall(ToolCall {
                    id: "call_1".into(),
                    name: "bash".into(),
                    arguments: serde_json::json!({"command": "ls"}),
                    thought_signature: None,
                    namespace: None,
                }),
            ],
            api: "anthropic-messages".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-5".into(),
            response_model: Some("claude-sonnet-4-5-20250929".into()),
            response_id: Some("msg_01ABC".into()),
            provider_thinking_level: Some("high".into()),
            diagnostics: Some(vec![AssistantMessageDiagnostic {
                r#type: "retry".into(),
                timestamp: 1758240000000,
                error: Some(DiagnosticErrorInfo {
                    name: Some("HttpError".into()),
                    message: "429 too many requests".into(),
                    stack: Some("at f()".into()),
                    code: Some(DiagnosticCode::Number(serde_json::Number::from(429))),
                }),
                details: Some(serde_json::json!({"attempt": 1})),
            }]),
            usage: Usage {
                input: 10,
                output: 5,
                cache_read: 0,
                cache_write: 0,
                cache_write_1h: Some(0),
                reasoning: Some(5),
                total_tokens: 15,
                cost: UsageCost {
                    input: 0.01,
                    output: 0.02,
                    cache_read: 0.0,
                    cache_write: 0.0,
                    total: 0.03,
                },
            },
            stop_reason: StopReason::ToolUse,
            deferred: Some(DeferredHandle {
                provider: "anthropic".into(),
                model_id: "claude-sonnet-4-5".into(),
                api: "anthropic-messages".into(),
                id: "resp_123".into(),
                expires_at: None,
                poll_after_ms: None,
                data: None,
            }),
            error_message: Some("first attempt failed".into()),
            raw_stop_reason: Some("tool_use".into()),
            end_turn: Some(false),
            timestamp: 1758240000004,
        };
        // The bare struct carries no role tag; the Message enum adds it.
        assert_eq!(
            serde_json::to_string(&Message::Assistant(assistant)).unwrap(),
            ASSISTANT_FULL
        );
    }

    #[test]
    fn assistant_message_minimal_round_trips_and_omits_optionals() {
        let msg: Message = serde_json::from_str(ASSISTANT_MINIMAL).unwrap();
        let Message::Assistant(assistant) = &msg else {
            panic!("expected assistant variant, got {msg:?}");
        };
        assert!(assistant.content.is_empty());
        assert_eq!(assistant.response_model, None);
        assert_eq!(assistant.response_id, None);
        assert_eq!(assistant.provider_thinking_level, None);
        assert_eq!(assistant.diagnostics, None);
        assert_eq!(assistant.deferred, None);
        assert_eq!(assistant.error_message, None);
        assert_eq!(assistant.raw_stop_reason, None);
        assert_eq!(assistant.end_turn, None);
        assert_eq!(serde_json::to_string(&msg).unwrap(), ASSISTANT_MINIMAL);

        let assistant = AssistantMessage {
            content: vec![],
            api: "openai-completions".into(),
            provider: "openai".into(),
            model: "gpt-5".into(),
            response_model: None,
            response_id: None,
            provider_thinking_level: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: TS + 5,
        };
        let json = serde_json::to_string(&assistant).unwrap();
        for key in [
            "responseModel",
            "responseId",
            "providerThinkingLevel",
            "diagnostics",
            "deferred",
            "errorMessage",
            "rawStopReason",
            "endTurn",
        ] {
            assert!(!json.contains(key), "{key} leaked: {json}");
        }
    }

    #[test]
    fn assistant_message_diagnostic_code_round_trips_as_string_and_number() {
        let string_code = r#"{"type":"warning","timestamp":1758240000000,"error":{"message":"connection reset","code":"ECONNRESET"}}"#;
        let diagnostic: AssistantMessageDiagnostic = serde_json::from_str(string_code).unwrap();
        assert_eq!(
            diagnostic.error.as_ref().unwrap().code,
            Some(DiagnosticCode::String("ECONNRESET".into()))
        );
        assert_eq!(serde_json::to_string(&diagnostic).unwrap(), string_code);

        let number_code = r#"{"type":"retry","timestamp":1758240000000,"error":{"name":"HttpError","message":"429","code":429}}"#;
        let diagnostic: AssistantMessageDiagnostic = serde_json::from_str(number_code).unwrap();
        assert_eq!(
            diagnostic.error.as_ref().unwrap().code,
            Some(DiagnosticCode::Number(serde_json::Number::from(429)))
        );
        assert_eq!(serde_json::to_string(&diagnostic).unwrap(), number_code);
    }

    #[test]
    fn tool_result_full_round_trips() {
        let msg: Message = serde_json::from_str(TOOL_RESULT_FULL).unwrap();
        let Message::ToolResult(result) = &msg else {
            panic!("expected toolResult variant, got {msg:?}");
        };
        assert_eq!(result.tool_call_id, "call_1");
        assert_eq!(result.tool_name, "bash");
        assert_eq!(
            result.content,
            vec![
                TextOrImageBlock::Text(TextContent {
                    text: "total 0".into(),
                    text_signature: None,
                }),
                TextOrImageBlock::Image(ImageContent {
                    data: "aGVsbG8=".into(),
                    mime_type: "image/png".into(),
                }),
            ]
        );
        assert_eq!(
            result.details,
            Some(serde_json::json!({"exitCode": 0, "files": ["a.txt"]}))
        );
        let usage = result.usage.as_ref().unwrap();
        assert_eq!(usage.input, 0);
        assert_eq!(usage.total_tokens, 0);
        assert!(!result.is_error);
        assert_eq!(serde_json::to_string(&msg).unwrap(), TOOL_RESULT_FULL);
    }

    #[test]
    fn tool_result_minimal_error_round_trips_and_omits_optionals() {
        let msg: Message = serde_json::from_str(TOOL_RESULT_MINIMAL_ERROR).unwrap();
        let Message::ToolResult(result) = &msg else {
            panic!("expected toolResult variant, got {msg:?}");
        };
        assert!(result.is_error);
        assert_eq!(result.details, None);
        assert_eq!(result.usage, None);
        assert_eq!(
            serde_json::to_string(&msg).unwrap(),
            TOOL_RESULT_MINIMAL_ERROR
        );

        let result = ToolResultMessage {
            tool_call_id: "call_2".into(),
            tool_name: "edit".into(),
            content: vec![TextOrImageBlock::Text(TextContent {
                text: "file not found".into(),
                text_signature: None,
            })],
            details: None,
            usage: None,
            is_error: true,
            timestamp: TS + 7,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(!json.contains("details"), "details leaked: {json}");
        assert!(!json.contains("usage"), "usage leaked: {json}");
    }

    #[test]
    fn string_or_blocks_round_trips_both_shapes() {
        let cases: Vec<(&str, StringOrBlocks)> = vec![
            (r#""plain text""#, StringOrBlocks::Text("plain text".into())),
            (
                r#"[{"type":"text","text":"a"}]"#,
                StringOrBlocks::Blocks(vec![TextOrImageBlock::Text(TextContent {
                    text: "a".into(),
                    text_signature: None,
                })]),
            ),
            (
                r#"[{"type":"image","data":"aGk=","mimeType":"image/png"}]"#,
                StringOrBlocks::Blocks(vec![TextOrImageBlock::Image(ImageContent {
                    data: "aGk=".into(),
                    mime_type: "image/png".into(),
                })]),
            ),
            (r#"[]"#, StringOrBlocks::Blocks(vec![])),
        ];
        for (wire, value) in cases {
            assert_eq!(serde_json::to_string(&value).unwrap(), wire);
            let back: StringOrBlocks = serde_json::from_str(wire).unwrap();
            assert_eq!(back, value);
        }
    }

    #[test]
    fn text_or_image_block_round_trips_both_variants() {
        let text = TextOrImageBlock::Text(TextContent {
            text: "hi".into(),
            text_signature: Some("sig".into()),
        });
        assert_eq!(
            serde_json::to_string(&text).unwrap(),
            r#"{"type":"text","text":"hi","textSignature":"sig"}"#
        );
        let back: TextOrImageBlock =
            serde_json::from_str(r#"{"type":"text","text":"hi","textSignature":"sig"}"#).unwrap();
        assert_eq!(back, text);

        let image = TextOrImageBlock::Image(ImageContent {
            data: "aGk=".into(),
            mime_type: "image/jpeg".into(),
        });
        assert_eq!(
            serde_json::to_string(&image).unwrap(),
            r#"{"type":"image","data":"aGk=","mimeType":"image/jpeg"}"#
        );
        let back: TextOrImageBlock =
            serde_json::from_str(r#"{"type":"image","data":"aGk=","mimeType":"image/jpeg"}"#)
                .unwrap();
        assert_eq!(back, image);
    }
}
