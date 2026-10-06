//! Agent core types from upstream `packages/agent/src/types.ts` (M3a Task 1):
//! the app-level [`AgentMessage`] union, the [`AgentEvent`] stream, tool
//! execution types ([`AgentTool`], [`AgentToolResult`], [`ToolExecutionMode`],
//! before/after hook results), [`AgentState`], and the data part of
//! `AgentOptions` (agent.ts:113-137) including `QueueMode` — the
//! one-at-a-time/all mode shared by the steering and follow-up queues.
//!
//! Wire format: `AgentMessage` is tagged by `role` and reuses the ai layer's
//! heavily tested `Message` serialization for the four standard roles, so
//! known wire fields keep their upstream shapes; standalone typed messages
//! do not retain unknown standard-role fields. AgentEvent JSON ingress retains
//! the original object alongside its validated typed view. The event `type`
//! values are the upstream snake_case literals (`agent_start`, ...,
//! `tool_execution_end`) and payload field names are camelCase
//! (`toolCallId`, `toolResults`, `assistantMessageEvent`, `partialResult`,
//! `isError`).
//!
//! Disclosed deviations from the TypeScript source:
//! - `CustomAgentMessages` (types.ts:324-326) is an open interface extended by
//!   declaration merging; a closed Rust enum cannot grow roles, so the
//!   extension point is the data-carrying [`AgentMessage::Custom`] variant:
//!   any object whose `role` is not one of the four standard roles captures as
//!   `{ role, data }` and serializes back to the same flat object. Apps add
//!   typed wrappers around `CustomAgentMessage` instead of interface merging.
//! - Upstream `AgentMessage = Message | CustomAgentMessages[...]` includes
//!   `SystemMessage`; the [`AgentMessage::System`] variant keeps that (the
//!   transcript's leading system message carries the prompt and tool
//!   declarations), even though LLM-facing code only consumes
//!   user/assistant/toolResult.
//! - `ThinkingLevel` (types.ts:308, the seven-value union including `"off"`)
//!   is exactly the ai layer's `ModelThinkingLevel`, so it is aliased, not
//!   redefined — one serde definition, one variant set.
//! - TypeBox generics (`TParameters`, `TDetails = any`) are erased to JSON at
//!   this boundary: tool arguments and `details` are `serde_json::Value`, the
//!   same precedent as `ToolResultMessage.details`. Typed tools deserialize on
//!   top (the loop validates before `execute`, mirroring upstream
//!   `validateToolCall`).
//! - Native `StreamFn` / `GetApiKeyFn` callbacks return poll-driven futures,
//!   not eager JavaScript promises. Stream callbacks receive the typed
//!   normalized transcript and a bounded receiver carries provider events.
//!   `AgentLoopConfig` and the loop hook types live in `agent_loop`.
//! - Upstream `AbortSignal` is the port's `tokio_util::sync::CancellationToken`
//!   (same convention as the ai layer's stream and retry surfaces).
//! - Custom-message payload insertion order is retained through serde_json
//!   preserve_order. The typed envelope emits the role field first.

mod event_wire;
pub use event_wire::PreservedAgentEvent;

use schemars::JsonSchema;
use serde::de::{Deserializer, Error as DeError};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::ai::types::content::ToolCall;
use crate::ai::types::events::AssistantMessageEvent;
use crate::ai::types::message::{
    AssistantMessage, Message, SystemMessage, TextOrImageBlock, ToolResultMessage, UserMessage,
};
use crate::ai::types::model::Model;
use crate::ai::types::primitives::{ModelCost, ThinkingBudgets, Usage};
use crate::ai::types::tool::{ConstrainedSampling, Tool};

/// Upstream `AgentToolCall` (types.ts:58): the `toolCall` content block an
/// assistant message emits, extracted from `AssistantMessage.content`.
pub type AgentToolCall = ToolCall;

/// Upstream agent `ThinkingLevel` (types.ts:308): `"off" | "minimal" | "low" |
/// "medium" | "high" | "xhigh" | "max"`. Identical union to the ai layer's
/// `ModelThinkingLevel` (ai types.ts:84), so alias rather than redefine — one
/// serde definition and variant set shared by both layers.
pub type ThinkingLevel = crate::ai::types::ModelThinkingLevel;

/// Upstream `ToolExecutionMode` (types.ts:47): how tool calls from a single
/// assistant message are executed.
///
/// - `Sequential`: each tool call is prepared, executed, and finalized before
///   the next one starts.
/// - `Parallel`: tool calls are prepared sequentially, then allowed tools
///   execute concurrently. `tool_execution_end` is emitted in tool completion
///   order after each tool is finalized, while tool-result message artifacts
///   are emitted later in assistant source order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolExecutionMode {
    Sequential,
    Parallel,
}

impl ToolExecutionMode {
    /// Upstream default (types.ts:275): `"parallel"`.
    pub const DEFAULT: Self = Self::Parallel;
}

/// Upstream `QueueMode` (types.ts:55): how many queued messages are injected
/// at a queue drain point. One type serves both queues — `AgentOptions`
/// `steeringMode` and `followUpMode` (agent.ts:131-132, the task brief's
/// SteeringMode/FollowUpMode).
///
/// - `All`: drain and inject every queued message at that point.
/// - `OneAtATime`: drain and inject only the oldest queued message, leaving
///   the rest queued for later drain points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QueueMode {
    All,
    OneAtATime,
}

impl QueueMode {
    /// Upstream default (README "Agent Options"): `"one-at-a-time"` for both
    /// the steering and the follow-up queue.
    pub const DEFAULT: Self = Self::OneAtATime;
}

/// Upstream `AgentTool.replay` (types.ts:422): recovery policy for an effect
/// whose durable intent exists but whose outcome is unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolReplay {
    Never,
    Safe,
}

/// Upstream `CustomAgentMessages` extension point (types.ts:324-326) as data:
/// any transcript object whose `role` is not one of the four standard roles.
///
/// Upstream apps register custom message kinds by declaration merging and
/// carry arbitrary fields on the message object. Here the role string plus the
/// remaining fields (as JSON) are captured verbatim, so a custom message
/// round-trips through the same wire shape upstream apps write
/// (`{"role":"notification","text":"Info","timestamp":...}`), and
/// `convertToLlm`-style code dispatches on [`AgentMessage::role`].
#[derive(Debug, Clone, PartialEq)]
pub struct CustomAgentMessage {
    /// The app-defined role discriminator (upstream `role`, e.g.
    /// `"notification"`).
    pub role: String,
    /// Every other field in insertion order (`serde_json/preserve_order`).
    /// This standalone typed envelope emits `role` first; raw event ingress
    /// preserves the original role position as well.
    pub data: serde_json::Map<String, serde_json::Value>,
}

impl CustomAgentMessage {
    /// An empty custom message with the given role.
    pub fn new(role: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            data: serde_json::Map::new(),
        }
    }
}

impl Serialize for CustomAgentMessage {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(1 + self.data.len()))?;
        map.serialize_entry("role", &self.role)?;
        for (key, value) in &self.data {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for CustomAgentMessage {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut role: Option<String> = None;
        let mut data = serde_json::Map::new();
        let value: serde_json::Value = Deserialize::deserialize(deserializer)?;
        let serde_json::Value::Object(entries) = value else {
            return Err(DeError::custom(
                "custom agent message must be a JSON object with a string \"role\"",
            ));
        };
        for (key, value) in entries {
            if key == "role" {
                role = Some(value.as_str().map(str::to_owned).ok_or_else(|| {
                    DeError::custom("custom agent message \"role\" must be a string")
                })?);
            } else {
                data.insert(key, value);
            }
        }
        Ok(Self {
            role: role.ok_or_else(|| DeError::missing_field("role"))?,
            data,
        })
    }
}

/// Upstream `AgentMessage` (types.ts:333): the app-level transcript union of
/// the four LLM messages plus custom app messages. This is the type the agent
/// stores, transforms (`transformContext`), and converts to LLM messages
/// (`convertToLlm`) before each provider call; LLMs only understand the
/// standard roles.
///
/// Serialization is tagged by `role` and delegates to the ai layer `Message`
/// impls for the standard roles. This standalone typed union is not a
/// lossless JSON carrier for unknown fields or original key order;
/// [`AgentEvent`] ingress preserves those separately.
///
/// The Assistant variant is intrinsically the largest payload (same reason
/// `types::Message` carries the same allow); boxing it would add indirection
/// at every use site for no functional gain.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum AgentMessage {
    /// Upstream `SystemMessage`: prompt and tool declarations. The leading
    /// system message is the prompt; later ones patch it.
    System(SystemMessage),
    /// Upstream `UserMessage`.
    User(UserMessage),
    /// Upstream `AssistantMessage`.
    Assistant(AssistantMessage),
    /// Upstream `ToolResultMessage`.
    ToolResult(ToolResultMessage),
    /// The `CustomAgentMessages[keyof CustomAgentMessages]` extension point,
    /// captured as data — see [`CustomAgentMessage`].
    Custom(CustomAgentMessage),
}

impl AgentMessage {
    /// The message's `role` string: the standard role literal or the custom
    /// role for [`Self::Custom`]. Mirrors dispatching on `message.role`
    /// upstream, including in `convertToLlm` implementations.
    pub fn role(&self) -> &str {
        match self {
            Self::System(_) => "system",
            Self::User(_) => "user",
            Self::Assistant(_) => "assistant",
            Self::ToolResult(_) => "toolResult",
            Self::Custom(custom) => &custom.role,
        }
    }

    /// The LLM-visible projection used by `convertToLlm` pass-through: the
    /// standard roles convert losslessly to their ai-layer message; custom
    /// messages are filtered out (`None`) — apps translate or drop them.
    pub fn to_message(&self) -> Option<Message> {
        match self {
            Self::System(m) => Some(Message::System(m.clone())),
            Self::User(m) => Some(Message::User(m.clone())),
            Self::Assistant(m) => Some(Message::Assistant(m.clone())),
            Self::ToolResult(m) => Some(Message::ToolResult(m.clone())),
            Self::Custom(_) => None,
        }
    }
}

impl From<Message> for AgentMessage {
    fn from(message: Message) -> Self {
        match message {
            Message::System(m) => Self::System(m),
            Message::User(m) => Self::User(m),
            Message::Assistant(m) => Self::Assistant(m),
            Message::ToolResult(m) => Self::ToolResult(m),
        }
    }
}

impl Serialize for AgentMessage {
    /// Standard roles delegate to the ai layer `Message` impls (the tested
    /// wire format, reused verbatim at the cost of one clone); custom messages
    /// serialize as their flat role-tagged object.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Custom(custom) => custom.serialize(serializer),
            Self::System(m) => Message::System(m.clone()).serialize(serializer),
            Self::User(m) => Message::User(m.clone()).serialize(serializer),
            Self::Assistant(m) => Message::Assistant(m.clone()).serialize(serializer),
            Self::ToolResult(m) => Message::ToolResult(m.clone()).serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for AgentMessage {
    /// Dispatches on the wire `role` string: the four standard roles parse as
    /// their ai-layer messages (full field validation, exact upstream wire
    /// compatibility), any other string role captures as a custom message —
    /// the runtime analogue of declaration merging. A missing or non-string
    /// `role` is a custom-message shape error, not a standard role.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        match value.get("role").and_then(serde_json::Value::as_str) {
            Some("system" | "user" | "assistant" | "toolResult") => Message::deserialize(value)
                .map(Self::from)
                .map_err(DeError::custom),
            _ => CustomAgentMessage::deserialize(value)
                .map(Self::Custom)
                .map_err(DeError::custom),
        }
    }
}

/// Upstream `AgentEvent` (types.ts:448-463): events emitted by the Agent for
/// UI updates. Wire `type` values are the upstream literals; payload field
/// names are camelCase.
///
/// `agent_end` is the last event emitted for a run, but awaited subscribers
/// for that event are still part of run settlement.
///
/// JSON decoding returns a preserving envelope. Use [`Self::kind`] when
/// matching typed events; `PartialEq` on the full enum also distinguishes
/// native and preserved representations. Serialization keeps ingress fields
/// and order, but standalone AgentMessage/provider/storage boundaries remain
/// typed and are not made lossless by this event wrapper.
///
/// The `MessageUpdate` variant is intrinsically the largest (partial assistant
/// message plus the assistant stream event, both hot-path values during
/// streaming); boxing either payload would add indirection on every streamed
/// event for no functional gain.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AgentEvent {
    /// Agent begins processing a run.
    AgentStart,
    /// Final event for a run; `messages` is the run's transcript snapshot.
    AgentEnd {
        /// Messages the run produced.
        messages: Vec<AgentMessage>,
    },
    /// A turn begins — one assistant response plus any tool calls/results.
    TurnStart,
    /// A turn completes.
    TurnEnd {
        /// The completed assistant message.
        message: AgentMessage,
        /// Tool result messages appended for the turn's tool calls.
        tool_results: Vec<ToolResultMessage>,
    },
    /// A message begins (system, user, assistant, or toolResult).
    MessageStart {
        /// The message that started.
        message: AgentMessage,
    },
    /// Assistant-only, during streaming; carries the underlying assistant
    /// stream event (delta, block start/end, ...).
    MessageUpdate {
        /// The partial assistant message.
        message: AgentMessage,
        /// The assistant message stream event driving the update.
        assistant_message_event: AssistantMessageEvent,
    },
    /// A message completes.
    MessageEnd {
        /// The completed message.
        message: AgentMessage,
    },
    /// A tool call starts executing.
    ToolExecutionStart {
        /// Id of the assistant toolCall block.
        tool_call_id: String,
        /// Tool name.
        tool_name: String,
        /// Validated tool arguments.
        args: serde_json::Value,
    },
    /// A tool streams a partial result (upstream `partialResult: any`; the
    /// producer passes an `AgentToolResult`-shaped value).
    ToolExecutionUpdate {
        /// Id of the assistant toolCall block.
        tool_call_id: String,
        /// Tool name.
        tool_name: String,
        /// Validated tool arguments.
        args: serde_json::Value,
        /// The partial result the tool streamed.
        partial_result: serde_json::Value,
    },
    /// A tool finished.
    ToolExecutionEnd {
        /// Id of the assistant toolCall block.
        tool_call_id: String,
        /// Tool name.
        tool_name: String,
        /// Final tool result (upstream `result: any`; the producer passes an
        /// `AgentToolResult`-shaped value).
        result: serde_json::Value,
        /// Whether the result is treated as an error.
        is_error: bool,
    },
    /// Validated JSON ingress, including unknown fields and original key order.
    /// Use [`Self::kind`] before pattern matching deserialized events.
    /// The carrier exposes no mutable typed view, so wire data cannot go stale.
    #[serde(untagged)]
    Preserved(PreservedAgentEvent),
}

/// Upstream `BeforeToolCallResult` (types.ts:66-74): returned from the
/// `beforeToolCall` hook. `block: true` prevents execution — the loop emits an
/// error tool result instead, using `reason` as its text (a default blocked
/// message when omitted). `terminate: true` hints the agent should stop after
/// the current tool batch; early termination only happens when every finalized
/// tool result in the batch sets it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeforeToolCallResult {
    /// Prevent the tool from executing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block: Option<bool>,
    /// Text used in the emitted error tool result when blocked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Participate in the batch early-termination rule (blocked call).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminate: Option<bool>,
}

/// Upstream `AfterToolCallResult` (types.ts:89-100): partial override returned
/// from the `afterToolCall` hook. Merge semantics are field-by-field — a
/// `Some` field replaces the executed tool result's value in full (`content`
/// replaces the whole content array, `details` the whole details value,
/// etc.); `None` keeps the original. There is no deep merge.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AfterToolCallResult {
    /// Replaces the tool result content array in full.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<TextOrImageBlock>>,
    /// Replaces the tool result details value in full.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::serde_support::present_json"
    )]
    pub details: Option<serde_json::Value>,
    /// Replaces the tool result error flag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    /// Replaces the tool result usage (execution usage only; not main LLM
    /// context accounting).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Replaces the early-termination hint for the batch rule.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminate: Option<bool>,
}

/// Upstream `AgentToolResult` (types.ts:383-395, at its default
/// `TDetails = JsonValue | undefined`): final or partial result produced by a
/// tool. `usage` is the tool execution's own usage, never main LLM context
/// accounting. `terminate: true` hints the agent should stop after the current
/// tool batch — effective only when every finalized result in the batch sets
/// it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolResult {
    /// Text or image content returned to the model.
    pub content: Vec<TextOrImageBlock>,
    /// Arbitrary structured details for logs or UI rendering.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::serde_support::present_json"
    )]
    pub details: Option<serde_json::Value>,
    /// Usage from the tool execution itself, if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Early-termination hint for the batch rule.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminate: Option<bool>,
}

/// Upstream `AgentToolUpdateCallback` (types.ts:403): scoped to the current
/// `execute()` invocation; calls after the tool settles are ignored by the
/// loop.
pub type AgentToolUpdateCallback = dyn Fn(&AgentToolResult) + Send + Sync;

/// Tool failure. Upstream `execute` throws on failure instead of encoding
/// errors in `content`; the agent catches and reports the error to the LLM as
/// a tool error with `isError: true`.
pub type BoxToolFuture = Pin<Box<dyn Future<Output = anyhow::Result<AgentToolResult>> + Send>>;

/// Upstream `AgentTool.prepareArguments` (types.ts:413): compatibility shim
/// applied to raw tool-call arguments before schema validation. Must return an
/// object matching the tool's parameter schema.
pub type PrepareArgumentsFn = dyn Fn(serde_json::Value) -> serde_json::Value + Send + Sync;

/// Upstream `AgentTool.execute` (types.ts:415-420) erased to JSON arguments:
/// `(toolCallId, params, signal, onUpdate)`. `params` are the validated
/// arguments; `signal` is the run's abort signal; `onUpdate` streams partial
/// results (`None` when nobody listens).
pub type ExecuteFn = dyn Fn(
        String,
        serde_json::Value,
        Option<CancellationToken>,
        Option<Arc<AgentToolUpdateCallback>>,
    ) -> BoxToolFuture
    + Send
    + Sync;

/// Upstream `AgentTool` (types.ts:406-431): a tool declaration (sent to the
/// LLM as part of the transcript's tool set) plus its executor and execution
/// policy.
///
/// `Clone` shares the executor hooks (`Arc`); `Debug` skips them;
/// `PartialEq` compares the declaration-visible fields and execution policy
/// only — function fields have no value equality.
#[derive(Clone)]
pub struct AgentTool {
    /// Tool name the model calls.
    pub name: String,
    /// Human-readable label for UI display (types.ts:408).
    pub label: String,
    /// Description sent to the model.
    pub description: String,
    /// JSON Schema object describing the parameters (upstream TypeBox
    /// `TSchema`).
    pub parameters: serde_json::Value,
    /// Optional constrained-sampling configuration (upstream
    /// `Tool.constrainedSampling`, ai types.ts:597).
    pub constrained_sampling: Option<ConstrainedSampling>,
    /// Execute the tool call. Throw (return `Err`) on failure instead of
    /// encoding errors in `content`.
    pub execute: Arc<ExecuteFn>,
    /// Optional compatibility shim for raw tool-call arguments before schema
    /// validation (types.ts:413).
    pub prepare_arguments: Option<Arc<PrepareArgumentsFn>>,
    /// Recovery policy for an effect whose durable intent exists but whose
    /// outcome is unknown (types.ts:422).
    pub replay: Option<ToolReplay>,
    /// Per-tool execution mode override (types.ts:424-430). If any tool call
    /// in a batch targets a tool with `Some(Sequential)`, the entire batch
    /// executes sequentially regardless of the global setting; `None` uses the
    /// default execution mode.
    pub execution_mode: Option<ToolExecutionMode>,
}

impl PartialEq for AgentTool {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.label == other.label
            && self.description == other.description
            && self.parameters == other.parameters
            && self.constrained_sampling == other.constrained_sampling
            && self.replay == other.replay
            && self.execution_mode == other.execution_mode
    }
}

impl fmt::Debug for AgentTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentTool")
            .field("name", &self.name)
            .field("label", &self.label)
            .field("description", &self.description)
            .field("parameters", &self.parameters)
            .field("constrained_sampling", &self.constrained_sampling)
            .field("prepare_arguments", &self.prepare_arguments.is_some())
            .field("replay", &self.replay)
            .field("execution_mode", &self.execution_mode)
            .finish_non_exhaustive()
    }
}

impl AgentTool {
    /// The tool declaration sent to providers / carried by transcript system
    /// messages (upstream `AgentTool extends Tool`).
    pub fn declaration(&self) -> Tool {
        Tool {
            name: self.name.clone(),
            description: self.description.clone(),
            parameters: self.parameters.clone(),
            constrained_sampling: self.constrained_sampling.clone(),
        }
    }
}

/// Convenience constructor for typed tools (the M1 `make_tool` helper,
/// carried into the core when the crate swapped onto it): the struct's JSON
/// Schema (schemars) is sent to the LLM, and incoming arguments are validated
/// by deserialization before `execute` runs. The loop validates every call
/// against the schema first (upstream `validateToolCall`); this second
/// deserialization also guards direct `execute` callers (unit tests, apps).
/// `label` defaults to the tool name.
pub fn make_tool<T, F>(name: &str, description: &str, execute: F) -> AgentTool
where
    T: serde::de::DeserializeOwned + JsonSchema + Send + 'static,
    F: Fn(T) -> BoxToolFuture + Send + Sync + 'static,
{
    AgentTool {
        name: name.to_string(),
        label: name.to_string(),
        description: description.to_string(),
        parameters: serde_json::to_value(schemars::schema_for!(T)).expect("schema serializes"),
        execute: Arc::new(move |_tool_call_id, value, _signal, _on_update| {
            match serde_json::from_value::<T>(value) {
                Ok(args) => execute(args),
                Err(error) => {
                    Box::pin(async move { Err(anyhow::anyhow!("invalid arguments: {error}")) })
                }
            }
        }),
        constrained_sampling: None,
        prepare_arguments: None,
        replay: None,
        execution_mode: None,
    }
}

/// Upstream `DEFAULT_MODEL` (agent.ts:55-69): placeholder model used until a
/// real model is assigned.
pub fn unknown_model() -> Model {
    Model {
        id: "unknown".into(),
        name: "unknown".into(),
        api: "unknown".into(),
        provider: "unknown".into(),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        sampling_params_by_thinking_level: None,
        input: Vec::new(),
        cost: ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        sampling_params: None,
        headers: None,
        compat: None,
        r#type: None,
        prompt_cache: None,
        input_limits: None,
    }
}

/// Upstream `AgentState` (types.ts:341-380) as data.
///
/// Upstream exposes `systemPrompt` (readonly, replayed from the transcript's
/// system messages), copy-on-assign `tools`/`messages` accessors, and
/// readonly runtime fields; as a Rust struct the fields are plain data —
/// replay, copying, and runtime mutation are the Agent's job (later tasks).
/// `pendingToolCalls` is a `BTreeSet` for deterministic iteration; the wire
/// shape is unaffected (the set is runtime-only). No `Default` derive: `Model`
/// has none — use [`AgentState::initial`].
#[derive(Debug, Clone, PartialEq)]
pub struct AgentState {
    /// Active model used for future turns.
    pub model: Model,
    /// Requested reasoning level for future turns.
    pub thinking_level: ThinkingLevel,
    /// Executable tools (the loadout; differences from the tools declared in
    /// the transcript are announced to the model with a system message before
    /// the next request).
    pub tools: Vec<Arc<AgentTool>>,
    /// Conversation transcript (system messages carry the prompt and tool
    /// declarations).
    pub messages: Vec<AgentMessage>,
    /// True while the agent is processing a prompt or continuation; remains
    /// true until awaited `agent_end` listeners settle.
    pub is_streaming: bool,
    /// Partial assistant message for the current streamed response, if any.
    pub streaming_message: Option<AgentMessage>,
    /// Tool call ids currently executing.
    pub pending_tool_calls: BTreeSet<String>,
    /// Error message from the most recent failed or aborted assistant turn,
    /// if any.
    pub error_message: Option<String>,
}

impl Default for AgentState {
    /// Upstream `createMutableAgentState` defaults (agent.ts:81-111): the
    /// unknown placeholder model, thinking level `"off"`, empty transcript.
    fn default() -> Self {
        Self {
            model: unknown_model(),
            thinking_level: ThinkingLevel::Off,
            tools: Vec::new(),
            messages: Vec::new(),
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: BTreeSet::new(),
            error_message: None,
        }
    }
}

impl AgentState {
    /// Upstream initial state defaults — see [`Default for AgentState`].
    pub fn initial() -> Self {
        Self::default()
    }
}

/// Upstream `AgentInitialState` (agent.ts:77-79): `Partial<Omit<AgentState,
/// runtime fields>>` plus `systemPrompt`, which seeds the leading system
/// message unless `messages` already starts with one (agent.ts:85-87).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AgentInitialState {
    /// Seeds the leading system message when `messages` does not start with
    /// one.
    pub system_prompt: Option<String>,
    /// Initial model; defaults to [`unknown_model`].
    pub model: Option<Model>,
    /// Initial thinking level; defaults to `off`.
    pub thinking_level: Option<ThinkingLevel>,
    /// Initial executable tools; become the leading system message's tool
    /// declarations.
    pub tools: Vec<Arc<AgentTool>>,
    /// Initial transcript.
    pub messages: Vec<AgentMessage>,
}

/// Upstream `StreamFn` (types.ts:33-37). The transcript is normalized before
/// invocation; the prompt and tool declarations live only in system messages.
/// An immediately available stream uses `Box::pin(async move { Ok(receiver) })`.
/// The factory may await setup without a thread or `block_on` bridge.
///
/// Request/model/runtime failures belong in terminal stream events. `Err`
/// represents a contract-violating throw/rejection: raw loops propagate it,
/// while `Agent` uses its existing error/aborted lifecycle choreography.
/// Cancellation is cooperative through `options.stream.signal`; the loop
/// does not race/drop the factory on abort (matching upstream's plain await).
pub type StreamFn = dyn Fn(
        Model,
        crate::ai::transcript::TranscriptContext,
        crate::ai::types::options::SimpleStreamOptions,
    ) -> futures::future::BoxFuture<
        'static,
        anyhow::Result<tokio::sync::mpsc::Receiver<AssistantMessageEvent>>,
    > + Send
    + Sync;

/// Upstream dynamic per-request API key resolver. Empty/absent results fall
/// back to the low-level stream option's explicit key (JavaScript `||`).
/// Like upstream, callers should return `None` rather than reject. `Err`
/// retains the raw-loop versus Agent failure distinction for invalid hooks.
pub type GetApiKeyFn = dyn Fn(String) -> futures::future::BoxFuture<'static, anyhow::Result<Option<String>>>
    + Send
    + Sync;

/// Upstream `AgentOptions` (agent.ts:113-138): the full Agent constructor
/// surface. The data fields landed with M3a Task 1; the callback/executor
/// fields attach with the loop (Task 3) and the Agent class (Task 4).
///
/// Port mapping: `stream_fn` and `get_api_key` are typed native callbacks;
/// `callbacks` carries upstream `onPayload`/`onResponse`. Without a custom
/// stream function the supplied `Models` collection remains the default.
/// Native futures are poll-driven rather than eager JS promises. `prepareNextTurn` and
/// `prepareNextTurnWithContext` are one hook here: the port's
/// [`PrepareNextTurnHook`] already receives the turn context. Upstream
/// forwards the active run's `AbortSignal` to
/// `beforeToolCall`/`afterToolCall`/`shouldStopAfterTurn`/
/// `prepareNextTurn`; the port's hook signatures have no signal parameter,
/// so that forwarding does not exist.
///
/// Mode defaults: steering/follow-up `one-at-a-time`, tool execution
/// `parallel`.
#[derive(Clone, Default)]
pub struct AgentOptions {
    /// Initial state; `systemPrompt` and `tools` become the leading system
    /// message unless `messages` already starts with one.
    pub initial_state: AgentInitialState,
    /// Optional native stream adapter; `None` retains the supplied Models fallback.
    pub stream_fn: Option<Arc<StreamFn>>,
    /// Resolved before every request, including subsequent tool/follow-up turns.
    pub get_api_key: Option<Arc<GetApiKeyFn>>,
    /// Upstream `onPayload` / `onResponse`, forwarded without serialization.
    pub callbacks: crate::ai::types::request_callbacks::RequestCallbacks,
    /// Upstream preferred transport; Agent defaults to `auto`.
    pub transport: Option<crate::ai::types::primitives::Transport>,
    /// Transcript-to-LLM conversion before each call (upstream
    /// `convertToLlm`); defaults to the standard-role filter.
    pub convert_to_llm: Option<std::sync::Arc<super::agent_loop::ConvertToLlmFn>>,
    /// Optional transcript transform before `convert_to_llm` (upstream
    /// `transformContext`).
    pub transform_context: Option<std::sync::Arc<super::agent_loop::TransformContextFn>>,
    /// Called before a tool executes, after argument validation (upstream
    /// `beforeToolCall`).
    pub before_tool_call: Option<std::sync::Arc<super::agent_loop::BeforeToolCallHook>>,
    /// Called after a tool finishes, before result events (upstream
    /// `afterToolCall`).
    pub after_tool_call: Option<std::sync::Arc<super::agent_loop::AfterToolCallHook>>,
    /// Called after `turn_end`; `true` stops the run (upstream
    /// `shouldStopAfterTurn`).
    pub should_stop_after_turn: Option<std::sync::Arc<super::agent_loop::ShouldStopAfterTurnHook>>,
    /// Called before the next turn when the loop continues (upstream
    /// `prepareNextTurn`/`prepareNextTurnWithContext`).
    pub prepare_next_turn: Option<std::sync::Arc<super::agent_loop::PrepareNextTurnHook>>,
    /// Steering queue drain mode (agent.ts:131); default
    /// [`QueueMode::DEFAULT`].
    pub steering_mode: Option<QueueMode>,
    /// Follow-up queue drain mode (agent.ts:132); default
    /// [`QueueMode::DEFAULT`].
    pub follow_up_mode: Option<QueueMode>,
    /// Session id for provider caching.
    pub session_id: Option<String>,
    /// Custom thinking budgets for token-based providers.
    pub thinking_budgets: Option<ThinkingBudgets>,
    /// Tool execution mode (agent.ts:136); default
    /// [`ToolExecutionMode::DEFAULT`].
    pub tool_execution: Option<ToolExecutionMode>,
    /// Upper bound on provider retry backoff.
    pub max_retry_delay_ms: Option<u64>,
}

impl std::fmt::Debug for AgentOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentOptions")
            .field("initial_state", &self.initial_state)
            .field("stream_fn", &self.stream_fn.is_some())
            .field("get_api_key", &self.get_api_key.is_some())
            .field("callbacks", &self.callbacks)
            .field("transport", &self.transport)
            .field("convert_to_llm", &self.convert_to_llm.is_some())
            .field("transform_context", &self.transform_context.is_some())
            .field("before_tool_call", &self.before_tool_call.is_some())
            .field("after_tool_call", &self.after_tool_call.is_some())
            .field(
                "should_stop_after_turn",
                &self.should_stop_after_turn.is_some(),
            )
            .field("prepare_next_turn", &self.prepare_next_turn.is_some())
            .field("steering_mode", &self.steering_mode)
            .field("follow_up_mode", &self.follow_up_mode)
            .field("session_id", &self.session_id)
            .field("thinking_budgets", &self.thinking_budgets)
            .field("tool_execution", &self.tool_execution)
            .field("max_retry_delay_ms", &self.max_retry_delay_ms)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::types::content::{ImageContent, TextContent};
    use crate::ai::types::message::StringOrBlocks;
    use crate::ai::types::primitives::StopReason;
    use std::sync::Mutex;

    const TS: i64 = 1758240000000;

    // Same fixtures as the ai layer message tests: serde_json Map keys sort
    // lexicographically (no preserve_order feature), so fixtures keep object
    // keys inside `serde_json::Value` fields sorted.
    const USER_BARE_STRING: &str =
        r#"{"role":"user","content":"hi there","timestamp":1758240000000}"#;

    const SYSTEM_BARE_STRING: &str =
        r#"{"role":"system","content":"You are pi.","timestamp":1758240000002}"#;

    const ASSISTANT_MINIMAL: &str = r#"{"role":"assistant","content":[],"api":"openai-completions","provider":"openai","model":"gpt-5","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"stopReason":"stop","timestamp":1758240000005}"#;

    const TOOL_RESULT_MINIMAL_ERROR: &str = r#"{"role":"toolResult","toolCallId":"call_2","toolName":"edit","content":[{"type":"text","text":"file not found"}],"isError":true,"timestamp":1758240000007}"#;

    fn text_block(text: &str) -> TextOrImageBlock {
        TextOrImageBlock::Text(TextContent {
            text: text.into(),
            text_signature: None,
        })
    }

    fn user_message(text: &str) -> AgentMessage {
        AgentMessage::User(UserMessage {
            content: StringOrBlocks::Text(text.into()),
            timestamp: TS,
        })
    }

    fn tool_result_message() -> ToolResultMessage {
        ToolResultMessage {
            tool_call_id: "call_2".into(),
            tool_name: "edit".into(),
            content: vec![text_block("file not found")],
            details: None,
            usage: None,
            is_error: true,
            timestamp: TS + 7,
        }
    }

    #[test]
    fn standard_message_roles_round_trip_byte_identically() {
        for wire in [
            USER_BARE_STRING,
            SYSTEM_BARE_STRING,
            ASSISTANT_MINIMAL,
            TOOL_RESULT_MINIMAL_ERROR,
        ] {
            let msg: AgentMessage = serde_json::from_str(wire).unwrap();
            assert_eq!(serde_json::to_string(&msg).unwrap(), wire, "wire: {wire}");
        }
    }

    #[test]
    fn standard_roles_deserialize_into_the_expected_variants() {
        let user: AgentMessage = serde_json::from_str(USER_BARE_STRING).unwrap();
        assert!(matches!(user, AgentMessage::User(_)));
        assert_eq!(user.role(), "user");

        let system: AgentMessage = serde_json::from_str(SYSTEM_BARE_STRING).unwrap();
        assert!(matches!(system, AgentMessage::System(_)));
        assert_eq!(system.role(), "system");

        let assistant: AgentMessage = serde_json::from_str(ASSISTANT_MINIMAL).unwrap();
        assert!(matches!(assistant, AgentMessage::Assistant(_)));
        assert_eq!(assistant.role(), "assistant");

        let tool_result: AgentMessage = serde_json::from_str(TOOL_RESULT_MINIMAL_ERROR).unwrap();
        assert!(matches!(tool_result, AgentMessage::ToolResult(_)));
        assert_eq!(tool_result.role(), "toolResult");
    }

    #[test]
    fn custom_message_round_trips_flat_object() {
        let wire = r#"{"role":"notification","text":"Info","timestamp":1758240000000}"#;
        let msg: AgentMessage = serde_json::from_str(wire).unwrap();
        let AgentMessage::Custom(custom) = &msg else {
            panic!("expected custom variant, got {msg:?}");
        };
        assert_eq!(custom.role, "notification");
        assert_eq!(custom.data.len(), 2);
        assert_eq!(custom.data["text"], "Info");
        assert_eq!(custom.data["timestamp"], 1758240000000i64);
        assert_eq!(msg.role(), "notification");
        assert_eq!(serde_json::to_string(&msg).unwrap(), wire);
    }

    #[test]
    fn custom_message_payload_keys_preserve_insertion_order() {
        // Arbitrary payloads retain their original key order, including nested data.
        let wire = r#"{"role":"artifact","zeta":1,"alpha":2}"#;
        let msg: AgentMessage = serde_json::from_str(wire).unwrap();
        let normalized = serde_json::to_string(&msg).unwrap();
        assert_eq!(normalized, wire);
        let back: AgentMessage = serde_json::from_str(&normalized).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), normalized);
    }

    #[test]
    fn custom_message_requires_object_with_string_role() {
        let missing: Result<AgentMessage, _> = serde_json::from_str(r#"{"text":"no role here"}"#);
        assert!(missing.is_err());

        let non_string: Result<AgentMessage, _> = serde_json::from_str(r#"{"role":42}"#);
        assert!(non_string.is_err());

        let non_object: Result<AgentMessage, _> = serde_json::from_str(r#""notification""#);
        assert!(non_object.is_err());

        // A custom message whose only field is the role is still valid.
        let bare: AgentMessage = serde_json::from_str(r#"{"role":"tick"}"#).unwrap();
        assert_eq!(serde_json::to_string(&bare).unwrap(), r#"{"role":"tick"}"#);
    }

    #[test]
    fn to_message_projects_standard_roles_and_filters_custom() {
        let user: AgentMessage = serde_json::from_str(USER_BARE_STRING).unwrap();
        assert!(matches!(user.to_message(), Some(Message::User(_))));

        let tool_result: AgentMessage = serde_json::from_str(TOOL_RESULT_MINIMAL_ERROR).unwrap();
        let Message::ToolResult(result) = tool_result.to_message().unwrap() else {
            panic!("expected toolResult message");
        };
        assert_eq!(result.tool_call_id, "call_2");
        assert!(result.is_error);

        let custom = AgentMessage::Custom(CustomAgentMessage::new("notification"));
        assert_eq!(custom.to_message(), None);
    }

    #[test]
    fn from_message_wraps_each_role() {
        let message: Message = serde_json::from_str(USER_BARE_STRING).unwrap();
        let agent_message = AgentMessage::from(message);
        assert!(matches!(agent_message, AgentMessage::User(_)));

        let message: Message = serde_json::from_str(TOOL_RESULT_MINIMAL_ERROR).unwrap();
        let agent_message = AgentMessage::from(message);
        assert!(matches!(agent_message, AgentMessage::ToolResult(_)));
    }

    #[test]
    fn agent_event_wire_shapes_match_types_ts() {
        // One fixture per upstream variant, in types.ts declaration order:
        // variant completeness and payload field names both pinned here.
        let message = user_message("Hello");
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
            timestamp: TS,
        };

        let cases: Vec<(AgentEvent, serde_json::Value)> = vec![
            (
                AgentEvent::AgentStart,
                serde_json::json!({"type":"agent_start"}),
            ),
            (
                AgentEvent::AgentEnd {
                    messages: vec![message.clone()],
                },
                serde_json::json!({"type":"agent_end","messages":[
                    {"role":"user","content":"Hello","timestamp":TS}
                ]}),
            ),
            (
                AgentEvent::TurnStart,
                serde_json::json!({"type":"turn_start"}),
            ),
            (
                AgentEvent::TurnEnd {
                    message: AgentMessage::Assistant(assistant),
                    tool_results: vec![tool_result_message()],
                },
                serde_json::json!({"type":"turn_end","message":serde_json::json!({"role":"assistant","content":[],"api":"openai-completions","provider":"openai","model":"gpt-5","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"stopReason":"stop","timestamp":TS}),"toolResults":[
                    // Bare ToolResultMessage, no role tag: upstream
                    // `toolResults: ToolResultMessage[]`, and per the ai-layer
                    // port the role tag lives on the Message enum only.
                    {"toolCallId":"call_2","toolName":"edit","content":[{"type":"text","text":"file not found"}],"isError":true,"timestamp":TS+7}
                ]}),
            ),
            (
                AgentEvent::MessageStart {
                    message: message.clone(),
                },
                serde_json::json!({"type":"message_start","message":{"role":"user","content":"Hello","timestamp":TS}}),
            ),
            (
                AgentEvent::MessageUpdate {
                    message: message.clone(),
                    assistant_message_event: AssistantMessageEvent::TextDelta {
                        content_index: 0,
                        delta: "Hi".into(),
                    },
                },
                serde_json::json!({"type":"message_update","message":{"role":"user","content":"Hello","timestamp":TS},"assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"Hi"}}),
            ),
            (
                AgentEvent::MessageEnd { message },
                serde_json::json!({"type":"message_end","message":{"role":"user","content":"Hello","timestamp":TS}}),
            ),
            (
                AgentEvent::ToolExecutionStart {
                    tool_call_id: "call_1".into(),
                    tool_name: "bash".into(),
                    args: serde_json::json!({"command":"ls"}),
                },
                serde_json::json!({"type":"tool_execution_start","toolCallId":"call_1","toolName":"bash","args":{"command":"ls"}}),
            ),
            (
                AgentEvent::ToolExecutionUpdate {
                    tool_call_id: "call_1".into(),
                    tool_name: "bash".into(),
                    args: serde_json::json!({"command":"ls"}),
                    partial_result: serde_json::json!({"content":[{"type":"text","text":"Reading..."}]}),
                },
                serde_json::json!({"type":"tool_execution_update","toolCallId":"call_1","toolName":"bash","args":{"command":"ls"},"partialResult":{"content":[{"type":"text","text":"Reading..."}]}}),
            ),
            (
                AgentEvent::ToolExecutionEnd {
                    tool_call_id: "call_1".into(),
                    tool_name: "bash".into(),
                    result: serde_json::json!({"content":[{"type":"text","text":"total 0"}]}),
                    is_error: false,
                },
                serde_json::json!({"type":"tool_execution_end","toolCallId":"call_1","toolName":"bash","result":{"content":[{"type":"text","text":"total 0"}]},"isError":false}),
            ),
        ];

        for (event, wire) in cases {
            let encoded = serde_json::to_value(&event).unwrap();
            assert_eq!(encoded, wire, "event: {event:?}");
            let decoded: AgentEvent = serde_json::from_value(wire).unwrap();
            assert_eq!(decoded.kind(), &event);
            assert_eq!(serde_json::to_value(&decoded).unwrap(), encoded);
        }
    }

    #[test]
    fn tool_execution_mode_wire_values_match_upstream() {
        assert_eq!(
            serde_json::to_string(&ToolExecutionMode::Parallel).unwrap(),
            r#""parallel""#
        );
        assert_eq!(
            serde_json::to_string(&ToolExecutionMode::Sequential).unwrap(),
            r#""sequential""#
        );
        let mode: ToolExecutionMode = serde_json::from_str(r#""parallel""#).unwrap();
        assert_eq!(mode, ToolExecutionMode::Parallel);
        assert_eq!(ToolExecutionMode::DEFAULT, ToolExecutionMode::Parallel);
    }

    #[test]
    fn queue_mode_wire_values_match_upstream() {
        assert_eq!(serde_json::to_string(&QueueMode::All).unwrap(), r#""all""#);
        assert_eq!(
            serde_json::to_string(&QueueMode::OneAtATime).unwrap(),
            r#""one-at-a-time""#
        );
        let mode: QueueMode = serde_json::from_str(r#""one-at-a-time""#).unwrap();
        assert_eq!(mode, QueueMode::OneAtATime);
        assert_eq!(QueueMode::DEFAULT, QueueMode::OneAtATime);
    }

    #[test]
    fn tool_replay_wire_values_match_upstream() {
        assert_eq!(
            serde_json::to_string(&ToolReplay::Never).unwrap(),
            r#""never""#
        );
        assert_eq!(
            serde_json::to_string(&ToolReplay::Safe).unwrap(),
            r#""safe""#
        );
        let replay: ToolReplay = serde_json::from_str(r#""safe""#).unwrap();
        assert_eq!(replay, ToolReplay::Safe);
    }

    #[test]
    fn agent_tool_result_omits_optional_fields_and_round_trips() {
        let result = AgentToolResult {
            content: vec![
                text_block("total 0"),
                TextOrImageBlock::Image(ImageContent {
                    data: "aGVsbG8=".into(),
                    mime_type: "image/png".into(),
                }),
            ],
            details: Some(serde_json::json!({"exitCode": 0})),
            usage: Some(Usage::default()),
            terminate: Some(true),
        };
        let wire = serde_json::to_value(&result).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({
                "content": [
                    {"type":"text","text":"total 0"},
                    {"type":"image","data":"aGVsbG8=","mimeType":"image/png"}
                ],
                "details": {"exitCode": 0},
                "usage": Usage::default(),
                "terminate": true
            })
        );
        let back: AgentToolResult = serde_json::from_value(wire).unwrap();
        assert_eq!(back, result);

        // Minimal result: optionals omitted, and missing optionals deserialize
        // as None (upstream `TDetails = JsonValue | undefined`).
        let minimal = AgentToolResult {
            content: vec![text_block("done")],
            ..AgentToolResult::default()
        };
        let wire = serde_json::to_value(&minimal).unwrap();
        assert!(!wire.to_string().contains("details"));
        assert!(!wire.to_string().contains("terminate"));
        let back: AgentToolResult = serde_json::from_value(wire).unwrap();
        assert_eq!(back, minimal);
    }

    #[test]
    fn before_and_after_tool_call_results_are_partial_data() {
        let blocked = BeforeToolCallResult {
            block: Some(true),
            reason: Some("bash is disabled".into()),
            terminate: Some(true),
        };
        assert_eq!(
            serde_json::to_value(&blocked).unwrap(),
            serde_json::json!({"block":true,"reason":"bash is disabled","terminate":true})
        );
        // Empty result = "no opinion": every field None, serializes as {}.
        assert_eq!(
            serde_json::to_value(BeforeToolCallResult::default()).unwrap(),
            serde_json::json!({})
        );

        let after = AfterToolCallResult {
            details: Some(serde_json::json!({"audited": true})),
            terminate: Some(true),
            ..AfterToolCallResult::default()
        };
        assert_eq!(after.content, None);
        assert_eq!(after.is_error, None);
        assert_eq!(
            serde_json::to_value(&after).unwrap(),
            serde_json::json!({"details":{"audited":true},"terminate":true})
        );
    }

    #[test]
    fn agent_state_initial_mirrors_upstream_defaults() {
        let state = AgentState::initial();
        assert_eq!(state.model.id, "unknown");
        assert_eq!(state.model.api, "unknown");
        assert_eq!(state.model.provider, "unknown");
        assert_eq!(state.model.context_window, 0);
        assert_eq!(state.model.max_tokens, 0);
        assert_eq!(state.model.input, Vec::new());
        assert_eq!(state.thinking_level, ThinkingLevel::Off);
        assert!(state.tools.is_empty());
        assert!(state.messages.is_empty());
        assert!(!state.is_streaming);
        assert_eq!(state.streaming_message, None);
        assert!(state.pending_tool_calls.is_empty());
        assert_eq!(state.error_message, None);
        assert_eq!(unknown_model().cost, ModelCost::default());
    }

    #[test]
    fn agent_options_and_initial_state_carry_data() {
        let options = AgentOptions::default();
        assert_eq!(options.steering_mode, None);
        assert_eq!(options.follow_up_mode, None);
        assert_eq!(options.tool_execution, None);
        assert_eq!(options.session_id, None);
        assert_eq!(options.max_retry_delay_ms, None);
        assert_eq!(options.initial_state, AgentInitialState::default());

        let tools = vec![Arc::new(echo_tool())];
        let options = AgentOptions {
            initial_state: AgentInitialState {
                system_prompt: Some("You are pi.".into()),
                model: Some(unknown_model()),
                thinking_level: Some(ThinkingLevel::High),
                tools: tools.clone(),
                messages: vec![user_message("Hello")],
            },
            stream_fn: None,
            get_api_key: None,
            callbacks: Default::default(),
            transport: None,
            convert_to_llm: None,
            transform_context: None,
            before_tool_call: None,
            after_tool_call: None,
            should_stop_after_turn: None,
            prepare_next_turn: None,
            steering_mode: Some(QueueMode::All),
            follow_up_mode: Some(QueueMode::OneAtATime),
            session_id: Some("session-123".into()),
            thinking_budgets: None,
            tool_execution: Some(ToolExecutionMode::Sequential),
            max_retry_delay_ms: Some(60000),
        };
        assert_eq!(
            options.initial_state.system_prompt,
            Some("You are pi.".into())
        );
        assert_eq!(
            options.initial_state.thinking_level,
            Some(ThinkingLevel::High)
        );
        assert_eq!(options.initial_state.tools.len(), 1);
        assert_eq!(options.steering_mode, Some(QueueMode::All));
        assert_eq!(options.tool_execution, Some(ToolExecutionMode::Sequential));

        // Plain data: Debug and Clone work across the tool-bearing state.
        let state = AgentState {
            tools,
            pending_tool_calls: BTreeSet::from(["call_1".into()]),
            ..AgentState::initial()
        };
        let debug = format!("{state:?}");
        assert!(debug.contains("AgentState"));
        assert!(debug.contains("call_1"));
        let cloned = state.clone();
        assert_eq!(cloned.pending_tool_calls, state.pending_tool_calls);
    }

    fn echo_tool() -> AgentTool {
        AgentTool {
            name: "echo".into(),
            label: "Echo".into(),
            description: "Echo the input".into(),
            parameters: serde_json::json!({"type":"object","properties":{"text":{"type":"string"}}}),
            execute: Arc::new(|tool_call_id, params, signal, on_update| {
                Box::pin(async move {
                    assert_eq!(tool_call_id, "call_1");
                    assert!(signal.is_none());
                    if let Some(on_update) = on_update {
                        on_update(&AgentToolResult {
                            content: vec![text_block("echoing...")],
                            ..AgentToolResult::default()
                        });
                    }
                    Ok(AgentToolResult {
                        content: vec![text_block(params["text"].as_str().unwrap_or(""))],
                        details: Some(serde_json::json!({"length": 5})),
                        usage: None,
                        terminate: None,
                    })
                })
            }),
            constrained_sampling: None,
            prepare_arguments: None,
            replay: None,
            execution_mode: Some(ToolExecutionMode::Sequential),
        }
    }

    #[tokio::test]
    async fn agent_tool_executes_streams_updates_and_exposes_declaration() {
        let updates = Arc::new(Mutex::new(Vec::<String>::new()));
        let writer = updates.clone();
        let on_update: Arc<AgentToolUpdateCallback> = Arc::new(move |partial| {
            let block = partial.content.first().cloned().unwrap();
            let TextOrImageBlock::Text(text) = block else {
                panic!("expected text block");
            };
            writer.lock().unwrap().push(text.text);
        });

        let tool = echo_tool();
        let result = (tool.execute)(
            "call_1".into(),
            serde_json::json!({"text": "hello"}),
            None,
            Some(on_update),
        )
        .await
        .unwrap();
        assert_eq!(result.content, vec![text_block("hello")]);
        assert_eq!(result.details, Some(serde_json::json!({"length": 5})));
        assert_eq!(result.terminate, None);
        assert_eq!(*updates.lock().unwrap(), vec!["echoing...".to_string()]);

        // Declaration mirrors the upstream `AgentTool extends Tool` surface.
        let declaration = tool.declaration();
        assert_eq!(declaration.name, "echo");
        assert_eq!(declaration.description, "Echo the input");
        assert_eq!(declaration.parameters, tool.parameters);
        assert_eq!(tool.execution_mode, Some(ToolExecutionMode::Sequential));
        assert!(format!("{tool:?}").contains("echo"));
    }

    #[test]
    fn agent_tool_prepare_arguments_shim_runs_before_validation() {
        let tool = AgentTool {
            prepare_arguments: Some(Arc::new(|mut args| {
                args["text"] = serde_json::Value::String("shimmed".into());
                args
            })),
            ..echo_tool()
        };
        let prepared =
            (tool.prepare_arguments.as_ref().unwrap())(serde_json::json!({"text": "raw"}));
        assert_eq!(prepared["text"], "shimmed");
        assert_eq!(tool.replay, None);
    }

    // ---- make_tool (the M1 typed-tool constructor, carried into the core) ----

    #[derive(serde::Deserialize, JsonSchema)]
    struct MakeToolArgs {
        text: String,
    }

    #[tokio::test]
    async fn make_tool_sends_the_schema_and_validates_by_deserialization() {
        let tool = make_tool("echo", "echo text back", |args: MakeToolArgs| {
            Box::pin(async move {
                Ok(AgentToolResult {
                    content: vec![text_block(&format!("echo: {}", args.text))],
                    ..AgentToolResult::default()
                })
            })
        });
        // The struct's schemars schema is the declaration sent to the LLM.
        assert_eq!(tool.name, "echo");
        assert_eq!(tool.label, "echo");
        assert_eq!(tool.description, "echo text back");
        assert_eq!(
            tool.parameters["properties"]["text"]["type"],
            serde_json::json!("string")
        );
        assert_eq!(tool.parameters["required"], serde_json::json!(["text"]));

        let ok = (tool.execute)(
            "call_1".into(),
            serde_json::json!({"text": "hi"}),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(ok.content, vec![text_block("echo: hi")]);

        // Invalid arguments become an "invalid arguments" error result.
        let error = (tool.execute)("call_2".into(), serde_json::json!({"wrong": 1}), None, None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("invalid arguments"), "{error}");
    }

    #[test]
    fn tool_result_and_hook_override_details_distinguish_absent_from_explicit_null() {
        use serde_json::{json, Value};
        fn round_trip<T: serde::de::DeserializeOwned + serde::Serialize>(wire: Value) {
            let restored: T = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(serde_json::to_value(restored).unwrap(), wire);
        }
        for details in [
            None,
            Some(Value::Null),
            Some(json!(false)),
            Some(json!({"nested":null})),
        ] {
            let mut result = json!({"content":[]});
            let mut hook = json!({});
            let mut message = json!({"toolCallId":"call","toolName":"tool","content":[],"isError":false,"timestamp":1});
            if let Some(details) = details {
                result["details"] = details.clone();
                hook["details"] = details.clone();
                message["details"] = details;
            }
            round_trip::<AgentToolResult>(result);
            round_trip::<AfterToolCallResult>(hook);
            round_trip::<crate::ai::types::message::ToolResultMessage>(message);
        }
    }
}
