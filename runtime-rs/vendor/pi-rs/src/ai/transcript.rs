//! Transcript semantics from upstream `packages/ai/src/utils/transcript.ts`
//! (snapshot commit 5901446) plus its two dependencies from
//! `packages/ai/src/utils/text.ts` (`contentText`, `getSystemMessageText`).
//!
//! The model: a transcript's system messages are patches. The leading system
//! message is the system prompt; every later one appends instructions
//! (`content`), replaces or removes named prompt sections (`sections`, where
//! `null` removes), and changes the tool set (`toolsAdded`/`toolsRemoved`).
//! Replaying all system messages in order yields the current prompt and
//! tools. `normalize_context` folds `Context.systemPrompt`/`Context.tools`
//! into a leading system message — the only entry point producing a
//! [`TranscriptContext`], which every provider-facing function expects.
//!
//! Section order is semantic (upstream types.ts:489-492 "Named, ordered
//! prompt sections"): the replay rebuilds `sections` in first-appearance
//! order, exactly like upstream's JS `Map` + `Object.fromEntries`
//! (transcript.ts:75-92). [`Sections`] preserves that order.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::ai::types::message::{
    Message, Sections, StringOrBlocks, SystemMessage, TextOrImageBlock,
};
use crate::ai::types::tool::{Tool, ToolReference};

/// Upstream `Context` (types.ts:610-614): the request input accepted by the
/// public stream entry points. `system_prompt` and `tools` are shorthand for
/// a leading system message; [`normalize_context`] folds them in before the
/// request reaches provider code. Plain serializable data, like upstream.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
}

/// Upstream `TranscriptContext` (types.ts:616-627): a normalized context whose
/// prompt and tool declarations are carried by the messages' system messages
/// alone. The field is `pub(crate)` so only this module (upstream: only
/// `normalizeContext`) can construct one; a raw [`Context`] cannot reach
/// provider code by accident.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TranscriptContext(pub(crate) Vec<Message>);

impl TranscriptContext {
    /// The normalized transcript messages.
    pub fn messages(&self) -> &[Message] {
        &self.0
    }
}

/// Upstream `createInitialSystemMessage` (transcript.ts:10-23): build the
/// leading system message for a prompt and tool set. Returns `None` when both
/// are empty, so an empty transcript stays empty. An empty-string prompt
/// counts as absent; an empty tool list counts as absent.
pub fn create_initial_system_message(
    system_prompt: Option<&str>,
    tools: Option<&[Tool]>,
) -> Option<SystemMessage> {
    let has_system_prompt = system_prompt.is_some_and(|prompt| !prompt.is_empty());
    let has_tools = tools.is_some_and(|tools| !tools.is_empty());
    if !has_system_prompt && !has_tools {
        return None;
    }
    Some(SystemMessage {
        content: StringOrBlocks::Text(system_prompt.unwrap_or_default().to_string()),
        sections: None,
        tools_added: has_tools.then(|| tools.unwrap_or_default().to_vec()),
        tools_removed: None,
        timestamp: 0,
    })
}

/// Upstream `normalizeContext` (transcript.ts:30-34): fold
/// `Context.systemPrompt` and `Context.tools` into a leading system message.
pub fn normalize_context(context: &Context) -> TranscriptContext {
    let initial =
        create_initial_system_message(context.system_prompt.as_deref(), context.tools.as_deref());
    let mut messages = Vec::with_capacity(context.messages.len() + usize::from(initial.is_some()));
    if let Some(head) = initial {
        messages.push(Message::System(head));
    }
    messages.extend(context.messages.iter().cloned());
    TranscriptContext(messages)
}

/// Upstream `getInitialSystemMessage` (transcript.ts:47-50): the leading
/// system message, if the transcript starts with one.
pub fn get_initial_system_message(messages: &[Message]) -> Option<&SystemMessage> {
    match messages.first() {
        Some(Message::System(system)) => Some(system),
        _ => None,
    }
}

/// Upstream `withoutInitialSystemMessage` (transcript.ts:53-55): drop the
/// leading system message for APIs that carry the prompt outside the message
/// list. Later system messages stay in place.
pub fn without_initial_system_message(messages: Vec<Message>) -> Vec<Message> {
    if get_initial_system_message(&messages).is_some() {
        messages.into_iter().skip(1).collect()
    } else {
        messages
    }
}

/// Upstream `getCurrentTools` (transcript.ts:58-66): resolve the tools
/// available after applying every transcript delta in order. Per system
/// message, removals apply before additions (so removing and re-adding a name
/// moves it to the end, like a JS `Map`); the result keeps first-declaration
/// order otherwise.
pub fn get_current_tools(messages: &[Message]) -> Vec<Tool> {
    let mut tools: Vec<(String, Tool)> = Vec::new();
    for message in messages {
        let Message::System(system) = message else {
            continue;
        };
        for reference in system.tools_removed.iter().flatten() {
            tools.retain(|(name, _)| name != &reference.name);
        }
        for tool in system.tools_added.iter().flatten() {
            ordered_set(&mut tools, tool.name.clone(), tool.clone());
        }
    }
    tools.into_iter().map(|(_, tool)| tool).collect()
}

/// Upstream `getCurrentSystemMessage` (transcript.ts:73-96): replay every
/// system message into one leading system message holding the current prompt
/// and tools. Later `content` is appended to the base prompt, `sections` are
/// patched by name (`null` removes), and tools are resolved with
/// [`get_current_tools`]. Returns `None` when the transcript has no system
/// messages and no resolvable tools. The replayed message always carries
/// string `content` (possibly empty), never `toolsRemoved`, its `sections`
/// and `toolsAdded` fields are omitted when empty, and the timestamp is the
/// first system message's (falling back to 0).
pub fn get_current_system_message(messages: &[Message]) -> Option<SystemMessage> {
    let mut content: Vec<String> = Vec::new();
    // JS `Map<string, string>`: null section values delete the name; set
    // keeps the first-appearance position (see `ordered_set`).
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut timestamp: Option<i64> = None;
    for message in messages {
        let Message::System(system) = message else {
            continue;
        };
        timestamp.get_or_insert(system.timestamp);
        let text = content_text(&system.content);
        if !text.is_empty() {
            content.push(text);
        }
        for (name, value) in system.sections.as_ref().into_iter().flatten() {
            match value {
                Some(text) => ordered_set(&mut sections, name.clone(), text.clone()),
                None => sections.retain(|(existing, _)| existing != name),
            }
        }
    }
    let tools = get_current_tools(messages);
    if timestamp.is_none() && tools.is_empty() {
        return None;
    }
    Some(SystemMessage {
        content: StringOrBlocks::Text(content.join("\n\n")),
        sections: (!sections.is_empty()).then(|| {
            Sections::new(
                sections
                    .into_iter()
                    .map(|(name, text)| (name, Some(text)))
                    .collect(),
            )
        }),
        tools_added: (!tools.is_empty()).then_some(tools),
        tools_removed: None,
        timestamp: timestamp.unwrap_or(0),
    })
}

/// Upstream `contentText` (text.ts:6-12) at its default `"\n"` separator:
/// extract and join the text blocks of message content; a bare string passes
/// through unchanged.
pub fn content_text(content: &StringOrBlocks) -> String {
    content_text_with_separator(content, "\n")
}

/// Upstream `contentText(content, separator)` (text.ts:6-12) with an explicit
/// separator (`""` where callers join blocks without one).
pub fn content_text_with_separator(content: &StringOrBlocks, separator: &str) -> String {
    match content {
        StringOrBlocks::Text(text) => text.clone(),
        StringOrBlocks::Blocks(blocks) => blocks
            .iter()
            .filter_map(|block| match block {
                TextOrImageBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<&str>>()
            .join(separator),
    }
}

/// Upstream `getSystemMessageText` (text.ts:15-21): render a system message
/// as a complete prompt — its content followed by its sections (in section
/// order), empty parts dropped, joined by blank lines. Section texts are
/// borrowed, not cloned, into the join.
pub fn get_system_message_text(message: &SystemMessage) -> String {
    let content = content_text(&message.content);
    let sections = message
        .sections
        .as_ref()
        .into_iter()
        .flatten()
        .filter_map(|(_, value)| value.as_deref())
        .filter(|text| !text.is_empty());
    std::iter::once(content.as_str())
        .chain(sections)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Upstream `getCurrentSystemPrompt` (transcript.ts:99-102): render the
/// current system prompt text after replaying every system message.
pub fn get_current_system_prompt(messages: &[Message]) -> String {
    get_current_system_message(messages)
        .map_or(String::new(), |message| get_system_message_text(&message))
}

/// Upstream `collapseSystemMessages` (transcript.ts:108-112): rebuild the
/// transcript for APIs without mid-conversation system messages — the
/// replayed system message leads, and every later system message is dropped.
pub fn collapse_system_messages(context: TranscriptContext) -> TranscriptContext {
    let head = get_current_system_message(&context.0);
    let mut messages: Vec<Message> = context
        .0
        .into_iter()
        .filter(|message| !matches!(message, Message::System(_)))
        .collect();
    if let Some(head) = head {
        messages.insert(0, Message::System(head));
    }
    TranscriptContext(messages)
}

/// Upstream `resolveTranscript` (transcript.ts:115-119): keep later system
/// messages in place when the model accepts them; otherwise collapse them.
/// Upstream's `boolean | undefined` maps to `Option<bool>`, where anything
/// but `Some(true)` collapses.
pub fn resolve_transcript(
    context: TranscriptContext,
    supports_mid_convo_system_messages: Option<bool>,
) -> TranscriptContext {
    if supports_mid_convo_system_messages == Some(true) {
        context
    } else {
        collapse_system_messages(context)
    }
}

/// Upstream `toToolDeclaration` (transcript.ts:123-130): strip executable and
/// display-only fields from a tool before transcript comparison or
/// persistence. Upstream round-trips `parameters` through
/// `JSON.parse(JSON.stringify(...))` to drop non-JSON values; in Rust
/// `serde_json::Value` already holds only JSON data, so the round-trip is
/// inherently a no-op and the clone is exact.
pub fn to_tool_declaration(tool: &Tool) -> Tool {
    Tool {
        name: tool.name.clone(),
        description: tool.description.clone(),
        parameters: tool.parameters.clone(),
        constrained_sampling: tool.constrained_sampling.clone(),
    }
}

/// Upstream `declarationsEqual` (transcript.ts:140-142): whether two tools
/// declare the same interface to the model, by comparing the serialized
/// declarations. Parameter object insertion order is significant, matching
/// upstream JSON.stringify; serde_json preserve_order retains that order.
/// Array order also stays significant.
pub fn declarations_equal(left: &Tool, right: &Tool) -> bool {
    // Tool serialization cannot fail (all fields are JSON data).
    let left_json =
        serde_json::to_string(&to_tool_declaration(left)).expect("tool declaration serializes");
    let right_json =
        serde_json::to_string(&to_tool_declaration(right)).expect("tool declaration serializes");
    left_json == right_json
}

/// Upstream `ToolStateChanges` (transcript.ts:144-148).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolStateChanges {
    pub tools_added: Vec<Tool>,
    pub tools_removed: Vec<ToolReference>,
}

/// Upstream `getToolStateChanges` (transcript.ts:150-167): compare two
/// complete tool states. A changed definition is a removal followed by an
/// addition. Additions keep `current` order; removals keep `previous` order.
pub fn get_tool_state_changes(previous: &[Tool], current: &[Tool]) -> ToolStateChanges {
    let previous_tools: HashMap<&str, &Tool> = previous
        .iter()
        .map(|tool| (tool.name.as_str(), tool))
        .collect();
    let current_tools: HashMap<&str, &Tool> = current
        .iter()
        .map(|tool| (tool.name.as_str(), tool))
        .collect();
    ToolStateChanges {
        tools_added: current
            .iter()
            .filter(|tool| {
                previous_tools
                    .get(tool.name.as_str())
                    .is_none_or(|previous| !declarations_equal(previous, tool))
            })
            .map(to_tool_declaration)
            .collect(),
        tools_removed: previous
            .iter()
            .filter(|tool| {
                current_tools
                    .get(tool.name.as_str())
                    .is_none_or(|current| !declarations_equal(tool, current))
            })
            .map(|tool| ToolReference {
                name: tool.name.clone(),
            })
            .collect(),
    }
}

/// Upstream `getDeclaredTools` (transcript.ts:170-177): every definition
/// referenced by transcript tool state, in first-declaration order (later
/// re-declarations replace the definition in place; removals are ignored).
pub fn get_declared_tools(messages: &[Message]) -> Vec<Tool> {
    let mut definitions: Vec<(String, Tool)> = Vec::new();
    for message in messages {
        let Message::System(system) = message else {
            continue;
        };
        for tool in system.tools_added.iter().flatten() {
            ordered_set(&mut definitions, tool.name.clone(), tool.clone());
        }
    }
    definitions.into_iter().map(|(_, tool)| tool).collect()
}

/// Upstream `hasToolRedefinitions` (transcript.ts:183-194): whether a tool
/// name was declared twice with different definitions. Transports that
/// reference tools by name (Anthropic `tool_addition`/`tool_removal`) cannot
/// express that; an identical re-declaration is fine.
pub fn has_tool_redefinitions(messages: &[Message]) -> bool {
    let mut declared: HashMap<&str, &Tool> = HashMap::new();
    for message in messages {
        let Message::System(system) = message else {
            continue;
        };
        for tool in system.tools_added.iter().flatten() {
            if let Some(previous) = declared.get(tool.name.as_str()) {
                if !declarations_equal(previous, tool) {
                    return true;
                }
            }
            declared.insert(tool.name.as_str(), tool);
        }
    }
    false
}

/// Upstream `hasNonAdditiveToolChanges` (transcript.ts:197-208): whether tool
/// history contains a removal or same-name redeclaration (by name alone,
/// regardless of definition equality) that an addition-only transport cannot
/// replay.
pub fn has_non_additive_tool_changes(messages: &[Message]) -> bool {
    let mut declared: HashSet<&str> = HashSet::new();
    for message in messages {
        let Message::System(system) = message else {
            continue;
        };
        if system
            .tools_removed
            .as_ref()
            .is_some_and(|references| !references.is_empty())
        {
            return true;
        }
        for tool in system.tools_added.iter().flatten() {
            if declared.contains(tool.name.as_str()) {
                return true;
            }
            declared.insert(tool.name.as_str());
        }
    }
    false
}

/// Upstream `TranscriptTools` (transcript.ts:210-218).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TranscriptTools {
    /// Tools sent in the top-level request field.
    pub request_tools: Vec<Tool>,
    /// Whether later system messages carry their own `toolsAdded` as in-place
    /// additions. When false, `request_tools` already holds the complete
    /// current tool set.
    pub anchors_additions: bool,
}

/// Upstream `resolveTranscriptTools` (transcript.ts:226-234): split tool
/// declarations between the top-level request field and in-place additions.
/// Transports that can anchor additions at a system message keep the initial
/// tools at the top and load later ones where they appear; that only works
/// when no tool was removed or redeclared, so everything else sends the
/// current tool list.
pub fn resolve_transcript_tools(
    messages: &[Message],
    supports_tool_additions: bool,
) -> TranscriptTools {
    let anchors_additions = supports_tool_additions && !has_non_additive_tool_changes(messages);
    let request_tools = if anchors_additions {
        get_initial_system_message(messages)
            .and_then(|message| message.tools_added.clone())
            .unwrap_or_default()
    } else {
        get_current_tools(messages)
    };
    TranscriptTools {
        request_tools,
        anchors_additions,
    }
}

/// JS `Map` insertion-order semantics used by the replay helpers: setting an
/// existing name replaces the value in place, a new name appends.
fn ordered_set<V>(entries: &mut Vec<(String, V)>, name: String, value: V) {
    match entries.iter_mut().find(|(existing, _)| *existing == name) {
        Some(slot) => slot.1 = value,
        None => entries.push((name, value)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::types::tool::{ConstrainedSampling, Disabled};
    use serde_json::json;

    const TS: i64 = 1758240000000;

    fn system_message(
        content: &str,
        timestamp: i64,
        sections: Option<Sections>,
        tools_added: Option<Vec<Tool>>,
        tools_removed: Option<Vec<ToolReference>>,
    ) -> Message {
        Message::System(SystemMessage {
            content: StringOrBlocks::Text(content.into()),
            sections,
            tools_added,
            tools_removed,
            timestamp,
        })
    }

    fn sys(content: &str, timestamp: i64) -> Message {
        system_message(content, timestamp, None, None, None)
    }

    fn user(content: &str, timestamp: i64) -> Message {
        Message::User(crate::ai::types::message::UserMessage {
            content: StringOrBlocks::Text(content.into()),
            timestamp,
        })
    }

    fn tool(name: &str, description: &str) -> Tool {
        Tool {
            name: name.into(),
            description: description.into(),
            parameters: json!({"type": "object"}),
            constrained_sampling: None,
        }
    }

    fn sections(pairs: &[(&str, Option<&str>)]) -> Sections {
        Sections::new(
            pairs
                .iter()
                .map(|(name, value)| ((*name).to_string(), value.map(|v| v.to_string())))
                .collect(),
        )
    }

    fn tool_names(tools: &[Tool]) -> Vec<&str> {
        tools.iter().map(|t| t.name.as_str()).collect()
    }

    // --- createInitialSystemMessage / normalizeContext (brief a, f) ---

    #[test]
    fn create_initial_system_message_cases() {
        // Both absent -> None, so an empty transcript stays empty.
        assert_eq!(create_initial_system_message(None, None), None);
        // Empty-string prompt counts as absent.
        assert_eq!(create_initial_system_message(Some(""), None), None);
        // Empty tool list counts as absent: message with prompt only.
        let prompt_only = create_initial_system_message(Some("Base."), Some(&[])).unwrap();
        assert_eq!(prompt_only.content, StringOrBlocks::Text("Base.".into()));
        assert_eq!(prompt_only.tools_added, None);
        assert_eq!(prompt_only.sections, None);
        assert_eq!(prompt_only.timestamp, 0);
        // Tools without a prompt: content is the empty string.
        let tools_only = create_initial_system_message(None, Some(&[tool("read", "r")])).unwrap();
        assert_eq!(tools_only.content, StringOrBlocks::Text(String::new()));
        assert_eq!(
            tool_names(tools_only.tools_added.as_ref().unwrap()),
            ["read"]
        );
        assert_eq!(tools_only.timestamp, 0);
        // Empty prompt + tools still creates the message for the tools.
        let both = create_initial_system_message(Some(""), Some(&[tool("read", "r")])).unwrap();
        assert_eq!(both.content, StringOrBlocks::Text(String::new()));
        assert_eq!(tool_names(both.tools_added.as_ref().unwrap()), ["read"]);
    }

    #[test]
    fn initial_system_message_wire_shape_omits_empty_tools() {
        let message = create_initial_system_message(Some("Base."), Some(&[])).unwrap();
        assert_eq!(
            serde_json::to_string(&message).unwrap(),
            r#"{"content":"Base.","timestamp":0}"#
        );
        let with_tools = create_initial_system_message(None, Some(&[tool("read", "r")])).unwrap();
        assert_eq!(
            serde_json::to_string(&with_tools).unwrap(),
            r#"{"content":"","toolsAdded":[{"name":"read","description":"r","parameters":{"type":"object"}}],"timestamp":0}"#
        );
    }

    #[test]
    fn normalize_context_folds_prompt_and_tools_into_leading_system_message() {
        let context = Context {
            system_prompt: Some("Base.".into()),
            messages: vec![user("hi", TS)],
            tools: Some(vec![tool("read", "r")]),
        };
        let transcript = normalize_context(&context);
        assert_eq!(transcript.messages().len(), 2);
        let Message::System(head) = &transcript.messages()[0] else {
            panic!("expected leading system message");
        };
        assert_eq!(head.content, StringOrBlocks::Text("Base.".into()));
        assert_eq!(tool_names(head.tools_added.as_ref().unwrap()), ["read"]);
        assert_eq!(head.timestamp, 0);
        assert_eq!(transcript.messages()[1], user("hi", TS));
    }

    #[test]
    fn normalize_context_empty_context_stays_empty() {
        assert!(normalize_context(&Context::default()).messages().is_empty());
        // Present-but-empty prompt and tools also add nothing.
        let context = Context {
            system_prompt: Some(String::new()),
            messages: vec![],
            tools: Some(vec![]),
        };
        assert!(normalize_context(&context).messages().is_empty());
    }

    #[test]
    fn normalize_context_keeps_existing_leading_system_message_first() {
        let context = Context {
            system_prompt: None,
            messages: vec![sys("custom prompt", TS), user("hi", TS + 1)],
            tools: None,
        };
        let transcript = normalize_context(&context);
        assert_eq!(transcript.messages(), &context.messages);
    }

    // --- getCurrentSystemMessage replay (brief b, c, d, e, f) ---

    #[test]
    fn no_system_messages_replay_to_none() {
        assert_eq!(get_current_system_message(&[]), None);
        assert_eq!(get_current_system_message(&[user("hi", TS)]), None);
    }

    #[test]
    fn later_content_appends_and_first_timestamp_wins() {
        let messages = [sys("Base.", 1), user("hi", 2), sys("Extra.", 3)];
        let replayed = get_current_system_message(&messages).unwrap();
        assert_eq!(
            replayed.content,
            StringOrBlocks::Text("Base.\n\nExtra.".into())
        );
        // The first system message's timestamp anchors the replay.
        assert_eq!(replayed.timestamp, 1);
    }

    #[test]
    fn sections_replace_by_name_and_null_removes() {
        let messages = [
            system_message(
                "Base.",
                1,
                Some(sections(&[("rules", Some("R1")), ("style", Some("S"))])),
                None,
                None,
            ),
            user("hi", 2),
            system_message(
                "",
                3,
                Some(sections(&[("rules", Some("R2")), ("style", None)])),
                None,
                None,
            ),
        ];
        let replayed = get_current_system_message(&messages).unwrap();
        let replayed_sections = replayed.sections.as_ref().unwrap();
        assert_eq!(replayed_sections.len(), 1);
        assert_eq!(replayed_sections.get("rules"), Some(&Some("R2".into())));
        assert_eq!(replayed_sections.get("style"), None);
        assert_eq!(get_system_message_text(&replayed), "Base.\n\nR2");
    }

    #[test]
    fn sections_removed_entirely_are_omitted_from_wire() {
        let messages = [
            system_message(
                "Base.",
                1,
                Some(sections(&[("rules", Some("R1"))])),
                None,
                None,
            ),
            system_message("", 2, Some(sections(&[("rules", None)])), None, None),
        ];
        let replayed = get_current_system_message(&messages).unwrap();
        assert_eq!(replayed.sections, None);
        assert_eq!(
            serde_json::to_string(&replayed).unwrap(),
            r#"{"content":"Base.","timestamp":1}"#
        );
    }

    #[test]
    fn section_order_is_first_appearance_not_lexicographic() {
        // The T3 named risk: upstream replays sections into a JS Map and emits
        // Object.fromEntries, so output key order is first-appearance order
        // (delete + re-add moves a name to the end), never sorted.
        let messages = [
            system_message(
                "Base.",
                1,
                Some(sections(&[("zeta", Some("Z")), ("alpha", Some("A"))])),
                None,
                None,
            ),
            system_message(
                "",
                2,
                Some(sections(&[("alpha", None), ("mid", Some("M"))])),
                None,
                None,
            ),
            system_message("", 3, Some(sections(&[("alpha", Some("A2"))])), None, None),
        ];
        let replayed = get_current_system_message(&messages).unwrap();
        let replayed_sections = replayed.sections.as_ref().unwrap();
        let names: Vec<&str> = replayed_sections
            .as_slice()
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        // zeta and mid keep their first-appearance positions; alpha was
        // removed and re-added, so it appends at the end.
        assert_eq!(names, ["zeta", "mid", "alpha"]);
        // Rendering follows the same order, so the prompt layout matches
        // upstream instead of lexicographic order.
        assert_eq!(
            get_current_system_prompt(&messages),
            "Base.\n\nZ\n\nM\n\nA2"
        );
        // Wire bytes keep the first-appearance order on write.
        assert_eq!(
            serde_json::to_string(&replayed).unwrap(),
            r#"{"content":"Base.","sections":{"zeta":"Z","mid":"M","alpha":"A2"},"timestamp":1}"#
        );
    }

    #[test]
    fn tools_added_and_removed_replay_in_map_order() {
        let messages = [
            system_message(
                "",
                1,
                None,
                Some(vec![tool("read", "r1"), tool("write", "w")]),
                None,
            ),
            user("hi", 2),
            system_message(
                "",
                3,
                None,
                Some(vec![tool("read", "r2")]),
                Some(vec![ToolReference {
                    name: "read".into(),
                }]),
            ),
        ];
        // Per message, removals apply before additions: "read" is deleted and
        // re-added, which appends it after "write" like a JS Map.
        let tools = get_current_tools(&messages);
        assert_eq!(tool_names(&tools), ["write", "read"]);
        assert_eq!(tools[1].description, "r2");
    }

    #[test]
    fn tool_removal_without_readdition_drops_the_tool() {
        let messages = [
            system_message(
                "",
                1,
                None,
                Some(vec![tool("read", "r"), tool("write", "w")]),
                None,
            ),
            system_message(
                "",
                2,
                None,
                None,
                Some(vec![ToolReference {
                    name: "read".into(),
                }]),
            ),
        ];
        let tools = get_current_tools(&messages);
        assert_eq!(tool_names(&tools), ["write"]);
        let replayed = get_current_system_message(&messages).unwrap();
        assert_eq!(
            tool_names(replayed.tools_added.as_ref().unwrap()),
            ["write"]
        );
        // The replay never carries toolsRemoved.
        assert_eq!(replayed.tools_removed, None);
    }

    #[test]
    fn content_blocks_join_within_a_message_and_messages_join_with_blank_lines() {
        let blocks = StringOrBlocks::Blocks(vec![
            TextOrImageBlock::Text(crate::ai::types::content::TextContent {
                text: "a".into(),
                text_signature: None,
            }),
            TextOrImageBlock::Text(crate::ai::types::content::TextContent {
                text: "b".into(),
                text_signature: None,
            }),
        ]);
        let messages = [
            Message::System(SystemMessage {
                content: blocks,
                sections: None,
                tools_added: None,
                tools_removed: None,
                timestamp: 1,
            }),
            sys("c", 2),
        ];
        let replayed = get_current_system_message(&messages).unwrap();
        assert_eq!(replayed.content, StringOrBlocks::Text("a\nb\n\nc".into()));
    }

    #[test]
    fn empty_content_pieces_are_skipped_but_replay_still_succeeds() {
        // Two empty system messages: timestamp is defined, so the replay is
        // Some with empty string content (not None).
        let messages = [sys("", 1), sys("", 2)];
        let replayed = get_current_system_message(&messages).unwrap();
        assert_eq!(replayed.content, StringOrBlocks::Text(String::new()));
        assert_eq!(replayed.timestamp, 1);
        assert_eq!(
            serde_json::to_string(&replayed).unwrap(),
            r#"{"content":"","timestamp":1}"#
        );
    }

    #[test]
    fn replayed_message_wire_shape_pins_field_order_and_omissions() {
        let messages = [
            sys("A.", 5),
            user("hi", 6),
            system_message("", 7, None, Some(vec![tool("read", "r")]), None),
        ];
        let replayed = get_current_system_message(&messages).unwrap();
        assert_eq!(
            serde_json::to_string(&replayed).unwrap(),
            r#"{"content":"A.","toolsAdded":[{"name":"read","description":"r","parameters":{"type":"object"}}],"timestamp":5}"#
        );
    }

    // --- getInitialSystemMessage / withoutInitialSystemMessage ---

    #[test]
    fn get_initial_system_message_cases() {
        let messages = [sys("A.", 1), user("hi", 2)];
        assert_eq!(
            get_initial_system_message(&messages).map(|m| &m.content),
            Some(&StringOrBlocks::Text("A.".into()))
        );
        let leading_user = [user("hi", 1), sys("A.", 2)];
        assert!(get_initial_system_message(&leading_user).is_none());
        assert!(get_initial_system_message(&[]).is_none());
    }

    #[test]
    fn without_initial_system_message_drops_only_the_leading_one() {
        let messages = vec![sys("A.", 1), user("hi", 2), sys("B.", 3)];
        let rest = without_initial_system_message(messages);
        assert_eq!(rest, vec![user("hi", 2), sys("B.", 3)]);

        // No leading system message: the list is returned untouched.
        let messages = vec![user("hi", 1), sys("A.", 2)];
        let rest = without_initial_system_message(messages.clone());
        assert_eq!(rest, messages);
    }

    // --- getCurrentSystemPrompt (README example) ---

    #[test]
    fn get_current_system_prompt_matches_readme_example() {
        // Ported verbatim from packages/ai/README.md "System Messages".
        let messages = [
            system_message(
                "You are helpful.",
                1,
                Some(sections(&[("rules", Some("<rules>Be brief.</rules>"))])),
                Some(vec![tool("read", "r")]),
                None,
            ),
            user("hi", 2),
            system_message(
                "",
                3,
                Some(sections(&[("rules", Some("<rules>Be thorough.</rules>"))])),
                None,
                Some(vec![ToolReference {
                    name: "read".into(),
                }]),
            ),
        ];
        assert_eq!(
            get_current_system_prompt(&messages),
            "You are helpful.\n\n<rules>Be thorough.</rules>"
        );
        assert!(get_current_tools(&messages).is_empty());
    }

    // --- collapseSystemMessages / resolveTranscript ---

    #[test]
    fn collapse_prepends_replayed_head_and_drops_all_system_messages() {
        let context = TranscriptContext(vec![sys("A.", 1), user("hi", 2), sys("B.", 3)]);
        let collapsed = collapse_system_messages(context);
        assert_eq!(collapsed.messages(), &[sys("A.\n\nB.", 1), user("hi", 2)]);

        // No system messages: the transcript is unchanged.
        let context = TranscriptContext(vec![user("hi", 1), user("ho", 2)]);
        let collapsed = collapse_system_messages(context);
        assert_eq!(collapsed.messages(), &[user("hi", 1), user("ho", 2)]);
    }

    #[test]
    fn resolve_transcript_collapses_unless_explicitly_supported() {
        let make = || TranscriptContext(vec![sys("A.", 1), user("hi", 2), sys("B.", 3)]);
        let kept = resolve_transcript(make(), Some(true));
        assert_eq!(
            kept.messages(),
            &[sys("A.", 1), user("hi", 2), sys("B.", 3)]
        );
        for unsupported in [Some(false), None] {
            let collapsed = resolve_transcript(make(), unsupported);
            assert_eq!(collapsed.messages(), &[sys("A.\n\nB.", 1), user("hi", 2)]);
        }
    }

    // --- tool declarations and comparisons ---

    #[test]
    fn to_tool_declaration_preserves_fields_and_constrained_sampling() {
        let plain = tool("read", "r");
        let declaration = to_tool_declaration(&plain);
        assert_eq!(declaration, plain);
        // constrained_sampling: None serializes omitted (upstream undefined).
        assert!(!serde_json::to_string(&declaration)
            .unwrap()
            .contains("constrainedSampling"));

        let disabled = Tool {
            constrained_sampling: Some(ConstrainedSampling::Disabled(Disabled)),
            ..plain.clone()
        };
        let declaration = to_tool_declaration(&disabled);
        assert_eq!(
            serde_json::to_string(&declaration).unwrap(),
            r#"{"name":"read","description":"r","parameters":{"type":"object"},"constrainedSampling":false}"#
        );
    }

    #[test]
    fn declarations_equal_cases() {
        let read = tool("read", "r");
        let read_again = tool("read", "r");
        assert!(declarations_equal(&read, &read_again));
        // Different description: a changed definition.
        assert!(!declarations_equal(&read, &tool("read", "r2")));
        // Different parameters value.
        let schema = Tool {
            parameters: json!({"type": "object", "properties": {}}),
            ..read.clone()
        };
        assert!(!declarations_equal(&read, &schema));
        // Different constrained sampling.
        let disabled = Tool {
            constrained_sampling: Some(ConstrainedSampling::Disabled(Disabled)),
            ..read.clone()
        };
        assert!(!declarations_equal(&read, &disabled));
        // Upstream JSON.stringify preserves parameter insertion order, so
        // reordered parameter objects declare different interfaces.
        let left: Tool = serde_json::from_str(
            r#"{"name":"read","description":"r","parameters":{"type":"object","properties":{}}}"#,
        )
        .unwrap();
        let right: Tool = serde_json::from_str(
            r#"{"name":"read","description":"r","parameters":{"properties":{},"type":"object"}}"#,
        )
        .unwrap();
        assert!(!declarations_equal(&left, &right));
        assert!(declarations_equal(&left, &left.clone()));
    }

    #[test]
    fn get_tool_state_changes_splits_additions_and_removals() {
        let previous = [tool("a", "a1"), tool("b", "b1")];
        let current = [tool("b", "b2"), tool("c", "c1")];
        let changes = get_tool_state_changes(&previous, &current);
        // "b" changed definition: a removal followed by an addition. Additions
        // keep current order; removals keep previous order.
        assert_eq!(tool_names(&changes.tools_added), ["b", "c"]);
        assert_eq!(changes.tools_added[0].description, "b2");
        let removed: Vec<&str> = changes
            .tools_removed
            .iter()
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(removed, ["a", "b"]);
        // No changes at all.
        let changes = get_tool_state_changes(&previous, &previous);
        assert!(changes.tools_added.is_empty());
        assert!(changes.tools_removed.is_empty());
    }

    #[test]
    fn get_declared_tools_keeps_first_declaration_order_and_replaces_in_place() {
        let messages = [
            system_message(
                "",
                1,
                None,
                Some(vec![tool("a", "a1"), tool("b", "b1")]),
                None,
            ),
            system_message(
                "",
                2,
                None,
                Some(vec![tool("a", "a2"), tool("c", "c1")]),
                None,
            ),
            system_message(
                "",
                3,
                None,
                None,
                Some(vec![ToolReference { name: "b".into() }]),
            ),
        ];
        let tools = get_declared_tools(&messages);
        assert_eq!(tool_names(&tools), ["a", "b", "c"]);
        // "a" keeps its first position but carries the later definition.
        assert_eq!(tools[0].description, "a2");
    }

    #[test]
    fn has_tool_redefinitions_cases() {
        let identical = [
            system_message("", 1, None, Some(vec![tool("a", "a1")]), None),
            system_message("", 2, None, Some(vec![tool("a", "a1")]), None),
        ];
        // An identical re-declaration is fine for declaration-comparing transports.
        assert!(!has_tool_redefinitions(&identical));

        let changed = [
            system_message("", 1, None, Some(vec![tool("a", "a1")]), None),
            system_message("", 2, None, Some(vec![tool("a", "a2")]), None),
        ];
        assert!(has_tool_redefinitions(&changed));

        let distinct = [system_message(
            "",
            1,
            None,
            Some(vec![tool("a", "a1"), tool("b", "b1")]),
            None,
        )];
        assert!(!has_tool_redefinitions(&distinct));
        assert!(!has_tool_redefinitions(&[]));
    }

    #[test]
    fn has_non_additive_tool_changes_cases() {
        let removal = [
            system_message("", 1, None, Some(vec![tool("a", "a1")]), None),
            system_message(
                "",
                2,
                None,
                None,
                Some(vec![ToolReference { name: "a".into() }]),
            ),
        ];
        assert!(has_non_additive_tool_changes(&removal));

        // Name-based: even an identical redeclaration is non-additive
        // (unlike has_tool_redefinitions).
        let redeclared = [
            system_message("", 1, None, Some(vec![tool("a", "a1")]), None),
            system_message("", 2, None, Some(vec![tool("a", "a1")]), None),
        ];
        assert!(has_non_additive_tool_changes(&redeclared));

        let additive = [
            system_message("", 1, None, Some(vec![tool("a", "a1")]), None),
            system_message("", 2, None, Some(vec![tool("b", "b1")]), None),
        ];
        assert!(!has_non_additive_tool_changes(&additive));
        assert!(!has_non_additive_tool_changes(&[]));
    }

    #[test]
    fn resolve_transcript_tools_cases() {
        let additive = [
            system_message("", 1, None, Some(vec![tool("a", "a1")]), None),
            system_message("", 2, None, Some(vec![tool("b", "b1")]), None),
        ];
        // Additive history with an anchoring transport: only the initial
        // tools go in the request field; later ones load in place.
        let resolved = resolve_transcript_tools(&additive, true);
        assert!(resolved.anchors_additions);
        assert_eq!(tool_names(&resolved.request_tools), ["a"]);

        // Non-additive history forces the full current tool list.
        let removal = [
            system_message("", 1, None, Some(vec![tool("a", "a1")]), None),
            system_message("", 2, None, Some(vec![tool("b", "b1")]), None),
            system_message(
                "",
                3,
                None,
                None,
                Some(vec![ToolReference { name: "a".into() }]),
            ),
        ];
        let resolved = resolve_transcript_tools(&removal, true);
        assert!(!resolved.anchors_additions);
        assert_eq!(tool_names(&resolved.request_tools), ["b"]);

        // No anchoring support: always the current tool list.
        let resolved = resolve_transcript_tools(&additive, false);
        assert!(!resolved.anchors_additions);
        assert_eq!(tool_names(&resolved.request_tools), ["a", "b"]);

        // Anchoring without a leading system message: empty request tools.
        let mid_only = [
            user("hi", 1),
            system_message("", 2, None, Some(vec![tool("b", "b1")]), None),
        ];
        let resolved = resolve_transcript_tools(&mid_only, true);
        assert!(resolved.anchors_additions);
        assert!(resolved.request_tools.is_empty());
    }

    // --- Context serialization ---

    #[test]
    fn context_round_trips_json_with_omitted_optionals() {
        let context = Context {
            system_prompt: Some("Base.".into()),
            messages: vec![user("hi", TS)],
            tools: Some(vec![tool("read", "r")]),
        };
        let wire = serde_json::to_string(&context).unwrap();
        assert_eq!(
            wire,
            r#"{"systemPrompt":"Base.","messages":[{"role":"user","content":"hi","timestamp":1758240000000}],"tools":[{"name":"read","description":"r","parameters":{"type":"object"}}]}"#
        );
        let back: Context = serde_json::from_str(&wire).unwrap();
        assert_eq!(back, context);

        // Missing optional fields deserialize to None; messages are required.
        let minimal: Context = serde_json::from_str(r#"{"messages":[]}"#).unwrap();
        assert_eq!(minimal.system_prompt, None);
        assert_eq!(minimal.tools, None);
        assert!(minimal.messages.is_empty());
    }
}
