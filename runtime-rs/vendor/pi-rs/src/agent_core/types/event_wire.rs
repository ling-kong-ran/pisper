//! Lossless JSON ingress for the agent event envelope. This is deliberately
//! not an unvalidated `Value` event: runtime consumers still see a valid kind.
use super::{AgentEvent, AgentMessage};
use crate::ai::types::{events::AssistantMessageEvent, message::ToolResultMessage};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// A validated event and its original JSON object. Fields are private; a
/// message replacement must update both through `replace_message_end`.
#[derive(Debug, Clone, PartialEq)]
pub struct PreservedAgentEvent {
    kind: Box<AgentEvent>,
    wire: Value,
}

impl Serialize for PreservedAgentEvent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.wire.serialize(serializer)
    }
}

// Remote derive constructs the public variants directly, without recursively
// invoking AgentEvent's preserving Deserialize implementation. This schema is
// only for validation; the original object is the serialization authority.
// Same layout as AgentEvent; this remote schema is never instantiated.
#[allow(clippy::large_enum_variant)]
#[derive(Deserialize)]
#[serde(
    remote = "AgentEvent",
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum AgentEventFields {
    AgentStart,
    AgentEnd {
        messages: Vec<AgentMessage>,
    },
    TurnStart,
    TurnEnd {
        message: AgentMessage,
        tool_results: Vec<ToolResultMessage>,
    },
    MessageStart {
        message: AgentMessage,
    },
    MessageUpdate {
        message: AgentMessage,
        assistant_message_event: AssistantMessageEvent,
    },
    MessageEnd {
        message: AgentMessage,
    },
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
        args: Value,
    },
    ToolExecutionUpdate {
        tool_call_id: String,
        tool_name: String,
        args: Value,
        partial_result: Value,
    },
    ToolExecutionEnd {
        tool_call_id: String,
        tool_name: String,
        result: Value,
        is_error: bool,
    },
}

impl<'de> Deserialize<'de> for AgentEvent {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = Value::deserialize(deserializer)?;
        let mut validation = wire.clone();
        if validation.get("type").and_then(Value::as_str) == Some("message_update") {
            if let Some(delta) = validation
                .get_mut("assistantMessageEvent")
                .and_then(Value::as_object_mut)
            {
                // The pi-ai reducer's internal Start uses `message`. Upstream
                // ingress uses `partial`; adapt only the typed view, including
                // when `message` is an unrelated extension field on the wire.
                if delta.get("type").and_then(Value::as_str) == Some("start") {
                    if let Some(partial) = delta.get("partial").cloned() {
                        delta.insert("message".into(), partial);
                    }
                }
            }
        }
        let kind = AgentEventFields::deserialize(validation).map_err(serde::de::Error::custom)?;
        Ok(Self::Preserved(PreservedAgentEvent {
            kind: Box::new(kind),
            wire,
        }))
    }
}

impl AgentEvent {
    /// Read-only typed view. Native Rust events return themselves; events
    /// decoded from JSON retain their original fields when serialized. Match
    /// on this view in reducers/listeners, not on the preservation envelope.
    pub fn kind(&self) -> &Self {
        match self {
            Self::Preserved(event) => event.kind.kind(),
            event => event,
        }
    }

    pub(crate) fn preserved_json(&self) -> Option<&Value> {
        match self {
            Self::Preserved(event) => Some(&event.wire),
            _ => None,
        }
    }

    /// Atomically replace a completed message, never merge it with the old
    /// snapshot. The new message's field order and unknown fields are kept;
    /// the enclosing event's original `message` slot stays in place.
    pub(crate) fn replace_message_end(
        &mut self,
        replacement: Value,
    ) -> Result<AgentMessage, serde_json::Error> {
        if !matches!(self.kind(), Self::MessageEnd { .. }) {
            return Err(<serde_json::Error as serde::de::Error>::custom(
                "not a message_end event",
            ));
        }
        let message: AgentMessage = serde_json::from_value(replacement.clone())?;
        let mut wire = serde_json::to_value(&*self)?;
        wire["message"] = replacement;
        *self = Self::Preserved(PreservedAgentEvent {
            kind: Box::new(Self::MessageEnd {
                message: message.clone(),
            }),
            wire,
        });
        Ok(message)
    }
}

#[cfg(test)]
#[path = "event_wire_tests.rs"]
mod tests;
