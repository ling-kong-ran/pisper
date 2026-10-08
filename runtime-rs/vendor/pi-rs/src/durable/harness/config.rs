//! Port of `src/harness/config.ts`: the durable per-conversation model,
//! thinking level, request options, and desired tool loadout, and the
//! built-in `pi.conversation.config` document.
//!
//! Divergence (structural, disclosed): the upstream state is a mutable
//! `Draft<ConversationConfigState>` document value; the port stores the same
//! JSON and converts to the typed [`ConversationConfigState`] view at read
//! sites, so the stored wire bytes (key order: `model?`, `thinkingLevel`,
//! `activeTools`, `streamOptions?`, `retry?`, `toolExecution?`,
//! `steeringMode?`, `followUpMode?`) are identical.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::super::documents::{define_doc, DefinitionScope, DocToken};
use super::super::errors::PlainError;
use super::super::types::{DocumentFork, DocumentHistory, JsonObject};
use super::types::{
    ConversationRetryPolicy, ConversationStreamOptions, QueueMode, ToolExecutionMode,
};

/// Durable per-conversation model, thinking level, request options, and
/// desired tool loadout (`config.ts` `ConversationConfigState`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationConfigState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<super::types::ModelRef>,
    pub thinking_level: crate::ai::types::ModelThinkingLevel,
    /// Desired tool names in offered order; names may be unregistered in the
    /// current process.
    pub active_tools: Vec<String>,
    /// Forwarded to every generation request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<ConversationStreamOptions>,
    /// Durable generation attempt retries; absent uses
    /// [`DEFAULT_RETRY_POLICY`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<ConversationRetryPolicy>,
    /// Whether a round's tools run at once or in call order; absent means
    /// `parallel`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_execution: Option<ToolExecutionMode>,
    /// How many queued steers a boundary places; absent means
    /// `one-at-a-time`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steering_mode: Option<QueueMode>,
    /// How many queued follow-ups a final boundary places; absent means
    /// `one-at-a-time`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_up_mode: Option<QueueMode>,
}

/// `DEFAULT_RETRY_POLICY` (`config.ts:23-28`).
pub fn default_retry_policy() -> ConversationRetryPolicy {
    ConversationRetryPolicy {
        enabled: true,
        max_retries: 3,
        base_delay_ms: 2000.0,
        max_agent_delay_ms: Some(60000.0),
    }
}

/// Built-in configuration document (`config.ts` `ConversationConfig`);
/// rewindable so forks start from the configuration at their fork entry.
/// Construction panics only on an invalid definition, which this fixed one is
/// not (upstream raises at module load).
pub fn conversation_config() -> DocToken {
    define_doc(super::super::documents::DocDefinition {
        kind: String::from("pi.conversation.config"),
        version: 1,
        scope: DefinitionScope::Conversation,
        history: Some(DocumentHistory::Rewindable),
        fork: Some(DocumentFork::AsOf),
        family: false,
        initial: Arc::new(|_| {
            ConversationConfigState::initial()
                .into_json()
                .expect("the initial config state serializes")
        }),
        migrate: None,
        checkpoint_when: Some(Arc::new(|_, _, _| true)),
    })
    .expect("the built-in configuration document definition is valid")
}

impl ConversationConfigState {
    /// Initial value (`config.ts:37`): `{ thinkingLevel: "off",
    /// activeTools: [] }`.
    pub fn initial() -> Self {
        ConversationConfigState {
            model: None,
            thinking_level: crate::ai::types::ModelThinkingLevel::Off,
            active_tools: Vec::new(),
            stream_options: None,
            retry: None,
            tool_execution: None,
            steering_mode: None,
            follow_up_mode: None,
        }
    }

    /// Round-trip through the stored JSON.
    pub fn into_json(self) -> Result<JsonObject, PlainError> {
        serde_json::to_value(self)
            .ok()
            .and_then(|value| value.as_object().cloned())
            .ok_or_else(|| {
                PlainError::new("ConversationConfigState does not serialize to an object")
            })
    }

    /// Parse the stored JSON; absent optional fields default per upstream
    /// spread semantics.
    pub fn from_json(value: &serde_json::Map<String, Value>) -> Result<Self, PlainError> {
        serde_json::from_value(Value::Object(value.clone()))
            .map_err(|error| PlainError::new(error.to_string()))
    }
}
