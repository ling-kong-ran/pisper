//! Session projection keeps the agent event's preserved JSON. The only outer
//! change is upstream's `{ ...event, willRetry }` on agent_end.
use super::{AgentEvent, AgentSessionEvent, Value};
use serde::{Serialize, Serializer};

/// Immutable typed/session-wire pair. Constructed only by the agent bridge.
#[derive(Debug, Clone, PartialEq)]
pub struct PreservedAgentSessionEvent {
    kind: Box<AgentSessionEvent>,
    wire: Value,
}

impl Serialize for PreservedAgentSessionEvent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.wire.serialize(serializer)
    }
}

impl AgentSessionEvent {
    /// Typed view for consumers of both native and JSON-ingress events.
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

    pub(crate) fn from_agent_event(event: &AgentEvent, will_retry: bool) -> Self {
        let kind = match event.kind() {
            AgentEvent::AgentStart => AgentSessionEvent::AgentStart,
            AgentEvent::AgentEnd { messages } => AgentSessionEvent::AgentEnd {
                messages: messages.clone(),
                will_retry,
            },
            AgentEvent::TurnStart => AgentSessionEvent::TurnStart,
            AgentEvent::TurnEnd {
                message,
                tool_results,
            } => AgentSessionEvent::TurnEnd {
                message: message.clone(),
                tool_results: tool_results.clone(),
            },
            AgentEvent::MessageStart { message } => AgentSessionEvent::MessageStart {
                message: message.clone(),
            },
            AgentEvent::MessageUpdate {
                message,
                assistant_message_event,
            } => AgentSessionEvent::MessageUpdate {
                message: message.clone(),
                assistant_message_event: serde_json::to_value(assistant_message_event)
                    .unwrap_or(Value::Null),
            },
            AgentEvent::MessageEnd { message } => AgentSessionEvent::MessageEnd {
                message: message.clone(),
            },
            AgentEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            } => AgentSessionEvent::ToolExecutionStart {
                tool_call_id: tool_call_id.clone(),
                tool_name: tool_name.clone(),
                args: args.clone(),
            },
            AgentEvent::ToolExecutionUpdate {
                tool_call_id,
                tool_name,
                args,
                partial_result,
            } => AgentSessionEvent::ToolExecutionUpdate {
                tool_call_id: tool_call_id.clone(),
                tool_name: tool_name.clone(),
                args: args.clone(),
                partial_result: partial_result.clone(),
            },
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                result,
                is_error,
            } => AgentSessionEvent::ToolExecutionEnd {
                tool_call_id: tool_call_id.clone(),
                tool_name: tool_name.clone(),
                result: result.clone(),
                is_error: *is_error,
            },
            AgentEvent::Preserved(_) => unreachable!("kind() unwraps ingress"),
        };
        if let Some(wire) = event.preserved_json() {
            let mut wire = wire.clone();
            if matches!(event.kind(), AgentEvent::AgentEnd { .. }) {
                // Map::insert overwrites in place or appends. Do not remove
                // first: an existing willRetry property keeps its key slot.
                wire.as_object_mut()
                    .expect("validated event object")
                    .insert("willRetry".into(), Value::Bool(will_retry));
            }
            Self::Preserved(PreservedAgentSessionEvent {
                kind: Box::new(kind),
                wire,
            })
        } else {
            kind
        }
    }
}
