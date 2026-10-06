//! Port of upstream `modes/json-event.ts`.
//!
//! `toJsonEvent` projects session events to the JSON/RPC stdout wire:
//! non-`message_update` events pass through untouched; `message_update`
//! collapses to `{type, usage, assistantMessageEvent}` with the cumulative
//! assistant snapshot removed. Upstream strips the shared `partial` field from
//! every assistant stream event and, for `toolcall_start`, inlines the
//! `id`/`toolName` of the tool call at `contentIndex` (read from
//! `event.partial.content[contentIndex]` when present; internal events use the
//! reconstructed live partial message carried by the enclosing AgentEvent).
//!
//! The internal start.message snapshot is removed at this wire boundary,
//! matching upstream start.partial removal without changing internal events.

use serde::Serialize;
use serde_json::Value;

use crate::agent_core::types::AgentMessage;
use crate::ai::types::message::AssistantBlock;
use crate::ai::types::message::AssistantMessage;
use crate::coding_agent::agent_session::AgentSessionEvent;

/// Upstream `JsonAgentSessionEvent`: the wire shape `toJsonEvent` produces.
/// The port works on the serialized [`Value`] projection; `message_update`
/// events are rebuilt with the exact upstream key order (`type`, `usage`,
/// `assistantMessageEvent`). Use [`to_json_event_string`] for JS Number wire
/// formatting; serde_json::to_string on the Value alone is not equivalent.
#[derive(Debug, Clone, PartialEq)]
pub struct JsonAgentSessionEvent(pub Value);

/// Upstream `toJsonAssistantMessageEvent`: drop the cumulative snapshot from
/// an assistant stream event and inline the tool-call identity for
/// `toolcall_start`.
///
fn to_json_assistant_message_event(
    event: &Value,
    assistant: &AssistantMessage,
) -> Result<Value, String> {
    let mut delta = event.clone();
    let Some(fields) = delta.as_object_mut() else {
        return Ok(delta);
    };
    let partial = fields.shift_remove("partial");
    let event_type = fields
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    // A valid numeric contentIndex is an array index. JSON.parse("0.0") is
    // the same JS Number as 0; normalize this known integer field, not all
    // arbitrary payload numbers (formatted at the final string boundary).
    if let Some(value) = fields.get_mut("contentIndex") {
        if value.is_number() {
            if let Some(index) = content_index(value) {
                *value = Value::from(index as u64);
            }
        }
    }
    match event_type.as_str() {
        "start" => {
            // Only the internal Start variant uses message instead of partial.
            // On an upstream-shaped event, message is an extra field to keep.
            if partial.is_none() {
                fields.shift_remove("message");
            }
        }
        "toolcall_start" => {
            let index = fields.get("contentIndex").and_then(content_index);
            let invalid_tool = || {
                let label = event
                    .get("contentIndex")
                    .map(|value| {
                        value
                            .as_str()
                            .map(str::to_owned)
                            .or_else(|| value.as_f64().map(crate::serde_support::js_number_string))
                            .unwrap_or_else(|| value.to_string())
                    })
                    .unwrap_or_else(|| "undefined".into());
                format!("toolcall_start content at index {label} is not a tool call")
            };
            if let Some(partial) = &partial {
                // Upstream reads the event's own partial, never event.message.
                let tool = partial
                    .get("content")
                    .and_then(Value::as_array)
                    .and_then(|content| index.and_then(|i| content.get(i)))
                    .filter(|tool| tool.get("type").and_then(Value::as_str) == Some("toolCall"))
                    .ok_or_else(invalid_tool)?;
                for (key, source) in [("id", "id"), ("toolName", "name")] {
                    if let Some(value) = tool.get(source) {
                        fields.insert(key.into(), value.clone());
                    } else {
                        // {...delta, id: undefined} overwrites then omits id.
                        fields.shift_remove(key);
                    }
                }
            } else {
                // Internal stream events carry the reconstructed live partial
                // on AgentEvent.message rather than serializing it per delta.
                let tool = index
                    .and_then(|i| assistant.content.get(i))
                    .and_then(|block| match block {
                        AssistantBlock::ToolCall(tool) => Some(tool),
                        _ => None,
                    })
                    .ok_or_else(invalid_tool)?;
                fields.insert("id".into(), Value::String(tool.id.clone()));
                fields.insert("toolName".into(), Value::String(tool.name.clone()));
            }
        }
        "done" | "error" => {
            let key = if event_type == "done" {
                "message"
            } else {
                "error"
            };
            if let Some(message) = fields.get_mut(key) {
                // Internal AssistantMessage lacks its enclosing role tag.
                // Never deserialize/rebuild an upstream-shaped payload: that
                // would discard unknown fields and change its existing order.
                if prepend_discriminator(message, "role", "assistant") {
                    if let Some(message) = message.as_object_mut() {
                        if let Some(error) = message.shift_remove("errorMessage") {
                            message.insert("errorMessage".into(), error);
                        }
                    }
                }
            }
        }
        "toolcall_end" => {
            if let Some(tool) = fields.get_mut("toolCall") {
                prepend_discriminator(tool, "type", "toolCall");
            }
        }
        _ => {}
    }
    Ok(delta)
}

fn content_index(value: &Value) -> Option<usize> {
    let index = if let Some(key) = value.as_str() {
        crate::serde_support::js_array_index(key)?
    } else {
        let number = value.as_f64()?;
        if !number.is_finite() || number < 0.0 || number >= u32::MAX as f64 || number.fract() != 0.0
        {
            return None;
        }
        number as u32
    };
    usize::try_from(index).ok()
}

/// Supply only the discriminator absent from an internal typed stream payload.
/// Already upstream-shaped objects keep their fields (including unknowns).
fn prepend_discriminator(value: &mut Value, key: &str, tag: &str) -> bool {
    let Some(fields) = value.as_object_mut() else {
        return false;
    };
    if fields.contains_key(key) {
        return false;
    }
    let mut tagged = serde_json::Map::with_capacity(fields.len() + 1);
    tagged.insert(key.into(), Value::String(tag.into()));
    tagged.extend(std::mem::take(fields));
    *fields = tagged;
    true
}

/// Upstream `{ type: "message_update", usage, assistantMessageEvent }`; the
/// explicit struct fixes the upstream key order.
#[derive(Debug, Serialize)]
struct MessageUpdateJsonEvent<'a> {
    r#type: &'static str,
    usage: &'a Value,
    #[serde(rename = "assistantMessageEvent")]
    assistant_message_event: &'a Value,
}

/// Upstream `toJsonEvent`: project a session event to the JSON wire shape.
///
/// Errors carry the exact upstream thrown messages (`message_update message
/// is not an assistant message` / ``toolcall_start content at index N is not
/// a tool call``).
pub fn to_json_event(event: &AgentSessionEvent) -> Result<Value, String> {
    let mut projected = if let AgentSessionEvent::MessageUpdate {
        message,
        assistant_message_event,
    } = event.kind()
    {
        let AgentMessage::Assistant(assistant) = message else {
            return Err("message_update message is not an assistant message".to_string());
        };
        let usage = if let Some(wire) = event.preserved_json() {
            wire["message"]["usage"].clone()
        } else {
            serde_json::to_value(assistant.usage).map_err(|error| error.to_string())?
        };
        let delta = event
            .preserved_json()
            .map(|wire| &wire["assistantMessageEvent"])
            .unwrap_or(assistant_message_event);
        let assistant_message_event = to_json_assistant_message_event(delta, assistant)?;
        serde_json::to_value(MessageUpdateJsonEvent {
            r#type: "message_update",
            usage: &usage,
            assistant_message_event: &assistant_message_event,
        })
        .map_err(|error| error.to_string())?
    } else {
        serde_json::to_value(event).map_err(|error| error.to_string())?
    };
    crate::serde_support::order_json_object_keys(&mut projected);
    Ok(projected)
}

/// Serialize the same projection returned by [`to_json_event`]. With
/// `serde_json/preserve_order`, arbitrary nested JSON keeps its string-key
/// insertion order; the projection applies JS integer-index enumeration.
/// Keep original stream JSON alongside typed validation: projecting only the
/// typed view reorders fields and discards nested unknowns. Direct session
/// Value deltas remain open; AgentEvent ingress validates its known variants.
/// This string boundary also applies JS binary64 number formatting;
/// ordinary serde_json serialization of the Value projection does not.
pub fn to_json_event_string(event: &AgentSessionEvent) -> Result<String, String> {
    crate::serde_support::to_json_string_with_js_numbers(&to_json_event(event)?)
        .map_err(|error| error.to_string())
}

/// Project an upstream-shaped event without discarding nested object order.
/// This raw upstream ingress keeps its original fields and uses the same JS
/// integer-index enumeration as the production Value boundary. Value interop
/// also preserves insertion order now that preserve_order is enabled.
pub fn project_ordered_event(
    event: &crate::coding_agent::core::model_config::OrderedValue,
) -> Result<crate::coding_agent::core::model_config::OrderedValue, String> {
    use crate::coding_agent::core::model_config::OrderedValue as O;
    if event.get("type").and_then(O::as_str) != Some("message_update") {
        let mut projected = event.clone();
        order_ordered_object_keys(&mut projected);
        return Ok(projected);
    }
    let message = event
        .get("message")
        .ok_or("message_update message is not an assistant message")?;
    if message.get("role").and_then(O::as_str) != Some("assistant") {
        return Err("message_update message is not an assistant message".into());
    }
    let delta = event
        .get("assistantMessageEvent")
        .ok_or("missing assistantMessageEvent")?;
    let O::Object(original) = delta else {
        return Err("assistantMessageEvent is not an object".into());
    };
    let mut fields: Vec<_> = original
        .iter()
        .filter(|(k, _)| k != "partial")
        .cloned()
        .collect();
    if delta.get("type").and_then(O::as_str) == Some("toolcall_start") {
        let index = delta.get("contentIndex");
        let numeric = index.and_then(|value| content_index(&value.to_serde()));
        let content = delta.get("partial").and_then(|p| p.get("content"));
        let tool = match (content, numeric) {
            (Some(O::Array(items)), Some(i)) => items.get(i),
            _ => None,
        };
        let tool = tool
            .filter(|v| v.get("type").and_then(O::as_str) == Some("toolCall"))
            .ok_or_else(|| {
                let label = index
                    .map(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .or_else(|| v.as_f64().map(crate::serde_support::js_number_string))
                            .unwrap_or_else(|| v.to_json_string())
                    })
                    .unwrap_or_else(|| "undefined".into());
                format!("toolcall_start content at index {label} is not a tool call")
            })?;
        for (key, source) in [("id", "id"), ("toolName", "name")] {
            if let Some(value) = tool.get(source) {
                if let Some((_, old)) = fields.iter_mut().find(|(k, _)| k == key) {
                    *old = value.clone();
                } else {
                    fields.push((key.into(), value.clone()));
                }
            } else {
                fields.retain(|(k, _)| k != key);
            }
        }
    }
    if let Some((_, value)) = fields.iter_mut().find(|(key, _)| key == "contentIndex") {
        let raw = value.to_serde();
        if raw.is_number() {
            if let Some(index) = content_index(&raw) {
                *value = O::Number((index as u64).into());
            }
        }
    }
    let mut output = vec![("type".into(), O::String("message_update".into()))];
    if let Some(usage) = message.get("usage") {
        output.push(("usage".into(), usage.clone()));
    }
    output.push(("assistantMessageEvent".into(), O::Object(fields)));
    let mut projected = O::Object(output);
    order_ordered_object_keys(&mut projected);
    Ok(projected)
}

fn order_ordered_object_keys(value: &mut crate::coding_agent::core::model_config::OrderedValue) {
    use crate::coding_agent::core::model_config::OrderedValue as O;
    match value {
        O::Array(items) => {
            for item in items {
                order_ordered_object_keys(item);
            }
        }
        O::Object(entries) => {
            crate::serde_support::order_js_object_entries(entries);
            for (_, value) in entries {
                order_ordered_object_keys(value);
            }
        }
        _ => {}
    }
}
