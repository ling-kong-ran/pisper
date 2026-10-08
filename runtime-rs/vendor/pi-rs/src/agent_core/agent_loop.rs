//! Agent loop from upstream `packages/agent/src/agent-loop.ts`:
//! prompt list -> per-turn context build (`declareToolChanges` +
//! `convertToLlm`) -> normalized typed stream adapter (or Models fallback) ->
//! assistant event mapping ->
//! tool execution (sequential and parallel modes with
//! before/after hooks and the batch early-termination rule) ->
//! toolResult messages -> repeat, with steering messages polled and injected
//! at the turn boundaries (agent-loop.ts:174, 200-204, 263), follow-up
//! messages checked when the agent would stop (agent-loop.ts:266-272), and
//! abort checks at the tool-execution points.
//!
//! The steering/follow-up queues themselves are abstracted behind the
//! `getSteeringMessages`/`getFollowUpMessages` hooks exactly as upstream;
//! [`PendingMessageQueue`] (the agent.ts queue the Agent class backs those
//! hooks with, including the one-at-a-time/all drain modes) ports alongside.
//!
//! Upstream is a generator-style `EventStream<AgentEvent, AgentMessage[]>`;
//! the port is channel-based: [`agent_loop`] spawns the loop and returns an
//! event receiver plus the run's `JoinHandle`, whose result is the run's new
//! messages (upstream `stream.result()`). [`run_agent_loop`]/[`run_agent_loop_continue`]
//! are the direct `emit`-sink equivalents of upstream `runAgentLoop`/
//! `runAgentLoopContinue`.
//!
//! Port deviations from the TypeScript source (each mirrors an existing
//! port ruling unless noted):
//! - The emit sink ([`AgentEventSink`]) is awaited by the loop at every
//!   emission, like upstream's awaited `emit` (which resolves when the
//!   EventStream's async handlers settle) — the README's raw-loop contract is
//!   observational because the *default* sink pushes onto a channel and
//!   returns immediately, while the Agent class (M3a Task 4) supplies a sink
//!   that awaits its listeners, making `message_end` a barrier before tool
//!   preflight (README "message_end barrier"). Parallel tool futures share
//!   the sink the way upstream's concurrent `emit` promises do.
//! - `beforeToolCall` receives the validated args and may return a
//!   replacement ([`BeforeToolCallOutcome::args`]). Upstream hooks mutate the
//!   shared `args` object in place and return nothing; a Rust closure cannot
//!   mutate through a shared value, so replacement is the same effect.
//!   Replacement args are executed without revalidation (upstream never
//!   revalidates after the hook; oracle
//!   "should execute mutated beforeToolCall args without revalidation").
//! - Hook and callback payloads carry owned snapshots (`AgentContext` clones),
//!   not live references: upstream hooks read the live loop context. Clones
//!   are read-equivalent for every hook contract in `types.ts`.
//! - Upstream `AbortSignal` is the port's [`CancellationToken`]; the loop
//!   plumbs it to the provider stream options and tool `execute` calls and
//!   checks it at the upstream points: tool preflight (after the
//!   `beforeToolCall` hook and before returning a prepared call), the
//!   sequential executor after each call, the parallel preflight after each
//!   entry, and the parallel execution closure (agent-loop.ts:531, 569, 597,
//!   691, 710, 576). A provider stream that settles aborted still ends the
//!   run through the error/aborted turn branch (agent-loop.ts:221-225).
//! - The M1-carried `max_turns` guard (upstream agent-loop.ts has no
//!   equivalent) bails the loop with `exceeded max_turns (N)` after emitting
//!   `agent_end`, preserving the agent_start..agent_end event pairing.
//! - A stream that closes without a terminal `done`/`error` event violates
//!   the upstream `StreamFn` contract ("Failures must be encoded in the
//!   returned stream"); the loop defends by settling with a synthesized
//!   error assistant message instead of hanging.
//! - Stream errors become the assistant message of a final aborted turn
//!   (`turn_end` + `agent_end`, agent-loop.ts:221-225); they are never
//!   converted to toolResult messages.
//! - Typed stream/key factories and SimpleStreamOptions are forwarded; when
//!   no factory is configured the supplied Models collection is the fallback
//!   instead of a process-global default. Factory/key `Err` values propagate
//!   from raw loops (upstream throw/reject); Agent catches them at its outer
//!   lifecycle boundary. Abort does not implicitly drop a pending factory.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::bail;
use futures::future::{join_all, BoxFuture};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::ai::models::{Models, ModelsSimpleStreamOptions};
use crate::ai::now_ms;
use crate::ai::transcript::{
    get_current_tools, get_tool_state_changes, normalize_context, Context, ToolStateChanges,
};
use crate::ai::types::content::{TextContent, ToolCall};
use crate::ai::types::events::{AssistantMessageEvent, PartialAssistant};
use crate::ai::types::message::{
    AssistantBlock, AssistantMessage, Message, StringOrBlocks, SystemMessage, TextOrImageBlock,
    ToolResultMessage,
};
use crate::ai::types::options::SimpleStreamOptions;
use crate::ai::types::primitives::{
    StopReason, ThinkingBudgets, ThinkingLevel as RequestThinkingLevel, Usage,
};
use crate::ai::validation::validate_tool_arguments;

use super::types::{
    AfterToolCallResult, AgentEvent, AgentMessage, AgentTool, AgentToolResult,
    AgentToolUpdateCallback, BeforeToolCallResult, GetApiKeyFn, QueueMode, StreamFn, ThinkingLevel,
    ToolExecutionMode,
};

/// Upstream `AgentContext` (types.ts:434-439): the context snapshot passed
/// into the low-level agent loop. `tools` is the executable loadout
/// (upstream optional; absent means empty).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentContext {
    /// Transcript visible to the model.
    pub messages: Vec<AgentMessage>,
    /// Tools available for execution in this run.
    pub tools: Vec<Arc<AgentTool>>,
}

/// The M1-carried max-turns guard tripped (upstream agent-loop.ts has no
/// equivalent). The run has already emitted its final `agent_end`, so
/// stateful callers ([`super::agent::Agent`]) treat this error as a settled
/// run instead of feeding it to a failure choreography: upstream maintains
/// exactly one `agent_end` per run on every path (agent-loop.ts:223, 259,
/// 278), and a synthetic failure message after it would corrupt the
/// transcript.
#[derive(Debug)]
pub struct MaxTurnsExceeded {
    /// The configured upper bound that was reached.
    pub max_turns: usize,
}

impl std::fmt::Display for MaxTurnsExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "exceeded max_turns ({})", self.max_turns)
    }
}

impl std::error::Error for MaxTurnsExceeded {}

/// The agent loop's event sink (upstream `AgentEventSink`, agent-loop.ts:31):
/// receives every [`AgentEvent`] in emission order. The loop awaits the sink
/// at each emission; the channel-based [`agent_loop`]/[`agent_loop_continue`]
/// wrappers pass a sink that pushes onto an unbounded channel and returns
/// immediately (an observational stream), while the Agent class passes one
/// that awaits its listeners. Shared so parallel tool futures can emit
/// concurrently, like upstream's concurrent `emit` promises.
pub type AgentEventSink = Arc<dyn Fn(AgentEvent) -> BoxFuture<'static, ()> + Send + Sync>;

/// Context passed to the `beforeToolCall` hook (upstream
/// `BeforeToolCallContext`, types.ts:103-112).
#[derive(Debug, Clone)]
pub struct BeforeToolCallContext {
    /// The assistant message that requested the tool call.
    pub assistant_message: AssistantMessage,
    /// The raw tool call block from `assistant_message.content`.
    pub tool_call: ToolCall,
    /// Validated tool arguments for the target tool schema.
    pub args: serde_json::Value,
    /// Agent context at the time the tool call is prepared (snapshot).
    pub context: AgentContext,
}

/// What a `beforeToolCall` hook returns (upstream
/// `BeforeToolCallResult | undefined` plus the in-place `args` mutation).
/// The default value is "no opinion".
#[derive(Debug, Clone, Default)]
pub struct BeforeToolCallOutcome {
    /// Replacement for the validated args (the port of upstream's in-place
    /// `args` mutation). Executed as-is, without revalidation.
    pub args: Option<serde_json::Value>,
    /// Block/terminate decision (upstream `BeforeToolCallResult`).
    pub result: Option<BeforeToolCallResult>,
}

/// Context passed to the `afterToolCall` hook (upstream
/// `AfterToolCallContext`, types.ts:115-128).
#[derive(Debug, Clone)]
pub struct AfterToolCallContext {
    /// The assistant message that requested the tool call.
    pub assistant_message: AssistantMessage,
    /// The raw tool call block from `assistant_message.content`.
    pub tool_call: ToolCall,
    /// Validated tool arguments for the target tool schema.
    pub args: serde_json::Value,
    /// The executed tool result before any overrides are applied.
    pub result: AgentToolResult,
    /// Whether the executed tool result is currently treated as an error.
    pub is_error: bool,
    /// Agent context at the time the tool call is finalized (snapshot).
    pub context: AgentContext,
}

/// Context passed to `shouldStopAfterTurn` and `prepareNextTurn` (upstream
/// `ShouldStopAfterTurnContext` / `PrepareNextTurnContext`, types.ts:131-140,
/// 154).
#[derive(Debug, Clone)]
pub struct ShouldStopAfterTurnContext {
    /// The assistant message that completed the turn.
    pub message: AssistantMessage,
    /// Tool result messages passed to the preceding `turn_end` event.
    pub tool_results: Vec<ToolResultMessage>,
    /// Agent context after the turn's assistant message and tool results were
    /// appended (snapshot).
    pub context: AgentContext,
    /// Messages this loop invocation returns if it exits at this point.
    pub new_messages: Vec<AgentMessage>,
}

/// Upstream `PrepareNextTurnContext` (types.ts:154) is the same shape.
pub type PrepareNextTurnContext = ShouldStopAfterTurnContext;

/// Upstream `AgentLoopTurnUpdate` (types.ts:143-152): replacement runtime
/// state used before starting another provider request.
#[derive(Debug, Clone, Default)]
pub struct AgentLoopTurnUpdate {
    /// Context for the next provider request; `None` keeps the current one.
    pub context: Option<AgentContext>,
    /// Messages to append before the next provider request.
    pub messages: Option<Vec<AgentMessage>>,
    /// Model for the next provider request; `None` keeps the current one.
    pub model: Option<crate::ai::types::Model>,
    /// Thinking level for the next request; `None` keeps the current one and
    /// `Some(off)` clears it (upstream maps `"off"` to `undefined`).
    pub thinking_level: Option<ThinkingLevel>,
}

/// `convertToLlm` (types.ts:156-185): converts the transcript to
/// LLM-compatible messages before each call. Must not fail (upstream
/// contract: return a safe fallback, never throw).
pub type ConvertToLlmFn =
    dyn Fn(Vec<AgentMessage>) -> BoxFuture<'static, Vec<Message>> + Send + Sync;

/// `transformContext` (types.ts:188-207): applied to the transcript before
/// `convertToLlm` (context-window management, external context injection).
pub type TransformContextFn =
    dyn Fn(Vec<AgentMessage>) -> BoxFuture<'static, Vec<AgentMessage>> + Send + Sync;

/// `beforeToolCall` (types.ts:279-285), called after argument validation.
pub type BeforeToolCallHook =
    dyn Fn(BeforeToolCallContext) -> BoxFuture<'static, BeforeToolCallOutcome> + Send + Sync;

/// `afterToolCall` (types.ts:288-300), called before `tool_execution_end` and
/// tool-result message events.
pub type AfterToolCallHook =
    dyn Fn(AfterToolCallContext) -> BoxFuture<'static, Option<AfterToolCallResult>> + Send + Sync;

/// `shouldStopAfterTurn` (types.ts:220-230), called after `turn_end`.
pub type ShouldStopAfterTurnHook =
    dyn Fn(ShouldStopAfterTurnContext) -> BoxFuture<'static, bool> + Send + Sync;

/// `prepareNextTurn` (types.ts:232-239), called after `turn_end` when the
/// loop will continue.
pub type PrepareNextTurnHook =
    dyn Fn(PrepareNextTurnContext) -> BoxFuture<'static, Option<AgentLoopTurnUpdate>> + Send + Sync;

/// `getSteeringMessages`/`getFollowUpMessages` (types.ts:243-265): polled by
/// the loop at the upstream points (loop start, after preparation, after each
/// turn; follow-up at the stop point). The Agent class (M3a Task 4) backs
/// these hooks with a [`PendingMessageQueue`].
pub type GetQueuedMessagesHook = dyn Fn() -> BoxFuture<'static, Vec<AgentMessage>> + Send + Sync;

/// Configuration for the low-level agent loop (upstream `AgentLoopConfig`,
/// types.ts:156-301). `max_turns` is the M1-carried guard.
pub struct AgentLoopConfig {
    /// Optional stream factory. `None` retains the supplied Models fallback.
    pub stream_fn: Option<Arc<StreamFn>>,
    /// Resolve the active model provider's key after context conversion, per turn.
    pub get_api_key: Option<Arc<GetApiKeyFn>>,
    /// Upstream inherited SimpleStreamOptions, including callbacks/transport.
    /// Existing typed thinking/session/retry fields below override when set;
    /// `thinking_level = Some(Off)` explicitly clears inherited reasoning.
    /// The active run's signal always replaces any signal stored here.
    pub stream_options: SimpleStreamOptions,
    /// Model used for provider requests.
    pub model: crate::ai::types::Model,
    /// Requested reasoning level; `off` maps to absent in the request
    /// (upstream `config.reasoning`).
    pub thinking_level: Option<ThinkingLevel>,
    /// Transcript-to-LLM conversion before each call (required upstream).
    pub convert_to_llm: Arc<ConvertToLlmFn>,
    /// Optional transcript transform before `convert_to_llm`.
    pub transform_context: Option<Arc<TransformContextFn>>,
    /// Tool execution mode; `None` uses the upstream default (parallel).
    pub tool_execution: Option<ToolExecutionMode>,
    /// Upper bound on assistant turns per run (M1-carried; upstream has no
    /// equivalent). Default 25.
    pub max_turns: usize,
    /// Session id forwarded to providers for cache-aware backends (upstream
    /// `AgentLoopConfig.sessionId`, passed through `SimpleStreamOptions`).
    pub session_id: Option<String>,
    /// Per-level thinking token budgets forwarded to the stream function
    /// (upstream `AgentLoopConfig.thinkingBudgets`).
    pub thinking_budgets: Option<ThinkingBudgets>,
    /// Optional cap for provider-requested retry delays (upstream
    /// `AgentLoopConfig.maxRetryDelayMs`).
    pub max_retry_delay_ms: Option<u64>,
    /// Called before a tool executes, after argument validation.
    pub before_tool_call: Option<Arc<BeforeToolCallHook>>,
    /// Called after a tool finishes, before result events.
    pub after_tool_call: Option<Arc<AfterToolCallHook>>,
    /// Called after `turn_end`; `true` stops the run.
    pub should_stop_after_turn: Option<Arc<ShouldStopAfterTurnHook>>,
    /// Called before the next turn when the loop continues.
    pub prepare_next_turn: Option<Arc<PrepareNextTurnHook>>,
    /// Steering queue hook, polled at loop start, after preparation, and
    /// after each completed turn (upstream `getSteeringMessages`).
    pub get_steering_messages: Option<Arc<GetQueuedMessagesHook>>,
    /// Follow-up queue hook, polled when the agent would stop (upstream
    /// `getFollowUpMessages`).
    pub get_follow_up_messages: Option<Arc<GetQueuedMessagesHook>>,
}

impl AgentLoopConfig {
    /// A config with the required fields and the upstream defaults:
    /// parallel tool execution, 25 max turns, no hooks.
    pub fn new(model: crate::ai::types::Model, convert_to_llm: Arc<ConvertToLlmFn>) -> Self {
        Self {
            stream_fn: None,
            get_api_key: None,
            stream_options: SimpleStreamOptions::default(),
            model,
            thinking_level: None,
            convert_to_llm,
            transform_context: None,
            tool_execution: None,
            max_turns: 25,
            session_id: None,
            thinking_budgets: None,
            max_retry_delay_ms: None,
            before_tool_call: None,
            after_tool_call: None,
            should_stop_after_turn: None,
            prepare_next_turn: None,
            get_steering_messages: None,
            get_follow_up_messages: None,
        }
    }
}

/// The channel pair [`agent_loop`] and [`agent_loop_continue`] return: the
/// event receiver plus the run handle (the run's new messages).
pub type AgentLoopRun = (
    mpsc::UnboundedReceiver<AgentEvent>,
    tokio::task::JoinHandle<anyhow::Result<Vec<AgentMessage>>>,
);

/// Upstream `PendingMessageQueue` (agent.ts:140-177): the steering/follow-up
/// message store behind the Agent class's `steer`/`followUp`/`clear*Queue`
/// surface. `drain` implements the two [`QueueMode`] behaviors — `All`
/// returns and clears the whole queue, `OneAtATime` returns only the oldest
/// message and leaves the rest for later drain points. The Agent class backs
/// its `getSteeringMessages`/`getFollowUpMessages` loop hooks with one queue
/// each; the loop itself only sees drained messages.
pub struct PendingMessageQueue {
    messages: Vec<AgentMessage>,
    mode: QueueMode,
}

impl PendingMessageQueue {
    /// An empty queue with the given drain mode.
    pub fn new(mode: QueueMode) -> Self {
        Self {
            messages: Vec::new(),
            mode,
        }
    }

    /// The current drain mode (upstream public `mode` field).
    pub fn mode(&self) -> QueueMode {
        self.mode
    }

    /// Change the drain mode (upstream assigning `queue.mode`).
    pub fn set_mode(&mut self, mode: QueueMode) {
        self.mode = mode;
    }

    /// Queue a message (upstream `enqueue`).
    pub fn enqueue(&mut self, message: AgentMessage) {
        self.messages.push(message);
    }

    /// Whether any message is queued (upstream `hasItems`).
    pub fn has_items(&self) -> bool {
        !self.messages.is_empty()
    }

    /// Take the messages injected at this drain point (upstream `drain`).
    pub fn drain(&mut self) -> Vec<AgentMessage> {
        match self.mode {
            QueueMode::All => std::mem::take(&mut self.messages),
            QueueMode::OneAtATime => {
                if self.messages.is_empty() {
                    Vec::new()
                } else {
                    vec![self.messages.remove(0)]
                }
            }
        }
    }

    /// Drop every queued message (upstream `clear`; the Agent class's
    /// `clearSteeringQueue`/`clearFollowUpQueue`/`clearAllQueues` primitive).
    pub fn clear(&mut self) {
        self.messages.clear();
    }
}

/// Upstream `agentLoop` (agent-loop.ts:37-60): start a run with new prompt
/// messages. Returns the event receiver and the run handle; the handle's
/// result is the run's new messages (upstream `stream.result()`).
pub fn agent_loop(
    prompts: Vec<AgentMessage>,
    context: AgentContext,
    config: AgentLoopConfig,
    models: Arc<Models>,
    signal: Option<CancellationToken>,
) -> AgentLoopRun {
    let (tx, rx) = mpsc::unbounded_channel();
    let sink: AgentEventSink = Arc::new(move |event| {
        let _ = tx.send(event);
        Box::pin(async {})
    });
    let handle = tokio::spawn(async move {
        run_agent_loop(prompts, context, config, models.as_ref(), signal, &sink).await
    });
    (rx, handle)
}

/// Upstream `agentLoopContinue` (agent-loop.ts:70-99): continue a run from
/// the current context without adding a message. The last context message
/// must convert to a `user` or `toolResult` message (caller responsibility;
/// custom messages are allowed).
pub fn agent_loop_continue(
    context: AgentContext,
    config: AgentLoopConfig,
    models: Arc<Models>,
    signal: Option<CancellationToken>,
) -> anyhow::Result<AgentLoopRun> {
    validate_continue_context(&context)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let sink: AgentEventSink = Arc::new(move |event| {
        let _ = tx.send(event);
        Box::pin(async {})
    });
    let handle = tokio::spawn(async move {
        run_agent_loop_continue(context, config, models.as_ref(), signal, &sink).await
    });
    Ok((rx, handle))
}

/// Upstream `runAgentLoop` (agent-loop.ts:101-125).
pub async fn run_agent_loop(
    prompts: Vec<AgentMessage>,
    context: AgentContext,
    config: AgentLoopConfig,
    models: &Models,
    signal: Option<CancellationToken>,
    emit: &AgentEventSink,
) -> anyhow::Result<Vec<AgentMessage>> {
    let initial_messages = declare_tool_changes(&context, prompts);
    let mut new_messages = initial_messages.clone();
    let mut messages = context.messages;
    messages.extend(initial_messages.iter().cloned());
    let mut current_context = AgentContext {
        messages,
        tools: context.tools,
    };

    (emit)(AgentEvent::AgentStart).await;
    (emit)(AgentEvent::TurnStart).await;
    for message in &initial_messages {
        (emit)(AgentEvent::MessageStart {
            message: message.clone(),
        })
        .await;
        (emit)(AgentEvent::MessageEnd {
            message: message.clone(),
        })
        .await;
    }

    run_loop(
        &mut current_context,
        &mut new_messages,
        config,
        models,
        signal,
        emit,
    )
    .await?;
    Ok(new_messages)
}

/// Upstream `runAgentLoopContinue` (agent-loop.ts:127-150).
pub async fn run_agent_loop_continue(
    context: AgentContext,
    config: AgentLoopConfig,
    models: &Models,
    signal: Option<CancellationToken>,
    emit: &AgentEventSink,
) -> anyhow::Result<Vec<AgentMessage>> {
    validate_continue_context(&context)?;
    let mut new_messages = Vec::new();
    let mut current_context = context;

    (emit)(AgentEvent::AgentStart).await;
    (emit)(AgentEvent::TurnStart).await;

    run_loop(
        &mut current_context,
        &mut new_messages,
        config,
        models,
        signal,
        emit,
    )
    .await?;
    Ok(new_messages)
}

/// Upstream agentLoopContinue/runAgentLoopContinue guards (agent-loop.ts:76-82,
/// 134-140).
fn validate_continue_context(context: &AgentContext) -> anyhow::Result<()> {
    if context.messages.is_empty() {
        bail!("Cannot continue: no messages in context");
    }
    if context.messages[context.messages.len() - 1].role() == "assistant" {
        bail!("Cannot continue from message role: assistant");
    }
    Ok(())
}

/// Upstream `runLoop` (agent-loop.ts:162-279): the outer loop re-enters when
/// queued follow-up messages arrive after the agent would stop; the inner
/// loop processes tool calls and steering messages.
async fn run_loop(
    current_context: &mut AgentContext,
    new_messages: &mut Vec<AgentMessage>,
    mut config: AgentLoopConfig,
    models: &Models,
    signal: Option<CancellationToken>,
    emit: &AgentEventSink,
) -> anyhow::Result<()> {
    let mut last_completed_turn: Option<PrepareNextTurnContext> = None;
    let mut turns_executed = 0usize;
    // Upstream checks for steering messages at start (agent-loop.ts:174):
    // the user may have typed while waiting for the run.
    let mut pending_messages: Vec<AgentMessage> = poll_steering(&config).await;

    // Outer loop: continues when queued follow-up messages arrive after the
    // agent would stop (agent-loop.ts:177-276).
    loop {
        let mut has_more_tool_calls = true;

        // Inner loop: process tool calls and steering messages
        // (agent-loop.ts:181-264).
        while has_more_tool_calls || !pending_messages.is_empty() {
            // M1-carried maxTurns guard (upstream agent-loop.ts has no
            // equivalent): the check sits before any turn-boundary work, so a
            // turn that would exceed the bound never starts. Emitting
            // agent_end first keeps the agent_start..agent_end pairing intact;
            // the typed [`MaxTurnsExceeded`] sentinel tells stateful callers
            // the run already settled, so they must not re-emit agent_end
            // through a failure path.
            if turns_executed >= config.max_turns {
                (emit)(AgentEvent::AgentEnd {
                    messages: new_messages.clone(),
                })
                .await;
                bail!(MaxTurnsExceeded {
                    max_turns: config.max_turns,
                });
            }

            let mut prepared_messages: Vec<AgentMessage> = Vec::new();
            if let Some(turn) = last_completed_turn {
                if let Some(hook) = &config.prepare_next_turn {
                    if let Some(update) = hook(turn).await {
                        if let Some(context) = update.context {
                            *current_context = context;
                        }
                        prepared_messages = update.messages.unwrap_or_default();
                        if let Some(model) = update.model {
                            config.model = model;
                        }
                        // Preserve an explicit Off until request construction so it
                        // also clears reasoning inherited from stream_options.
                        if let Some(level) = update.thinking_level {
                            config.thinking_level = Some(level);
                        }
                    }
                }
                // Preparation can be long-running (for example, compaction).
                // Pick up steering queued while it ran. Only poll again if the
                // earlier poll returned nothing; otherwise one-at-a-time mode
                // would deliver two messages in this turn
                // (agent-loop.ts:200-204).
                if pending_messages.is_empty() {
                    pending_messages = poll_steering(&config).await;
                }
                (emit)(AgentEvent::TurnStart).await;
            }

            // Process prepared and queued messages before the next assistant
            // response (agent-loop.ts:208-215).
            let incoming: Vec<AgentMessage> = prepared_messages
                .into_iter()
                .chain(pending_messages.drain(..))
                .collect();
            for message in declare_tool_changes(current_context, incoming) {
                (emit)(AgentEvent::MessageStart {
                    message: message.clone(),
                })
                .await;
                (emit)(AgentEvent::MessageEnd {
                    message: message.clone(),
                })
                .await;
                current_context.messages.push(message.clone());
                new_messages.push(message);
            }

            // Stream assistant response.
            let message =
                stream_assistant_response(current_context, &config, models, signal.clone(), emit)
                    .await?;
            turns_executed += 1;
            new_messages.push(AgentMessage::Assistant(message.clone()));

            // Stream errors end the run with the failed assistant message
            // (agent-loop.ts:221-225) — never toolResult messages.
            if message.stop_reason == StopReason::Error
                || message.stop_reason == StopReason::Aborted
            {
                (emit)(AgentEvent::TurnEnd {
                    message: AgentMessage::Assistant(message),
                    tool_results: Vec::new(),
                })
                .await;
                (emit)(AgentEvent::AgentEnd {
                    messages: new_messages.clone(),
                })
                .await;
                return Ok(());
            }

            let tool_calls: Vec<ToolCall> = message
                .content
                .iter()
                .filter_map(|block| match block {
                    AssistantBlock::ToolCall(call) => Some(call.clone()),
                    _ => None,
                })
                .collect();

            let mut tool_results: Vec<ToolResultMessage> = Vec::new();
            has_more_tool_calls = false;
            if !tool_calls.is_empty() {
                // A "length" stop means the output was cut off by the token
                // limit, so every tool call may carry truncated arguments
                // (agent-loop.ts:233-239).
                let batch = if message.stop_reason == StopReason::Length {
                    fail_tool_calls_from_truncated_message(&tool_calls, emit).await
                } else {
                    execute_tool_calls(current_context, &message, &config, signal.clone(), emit)
                        .await
                };
                tool_results = batch.messages;
                has_more_tool_calls = !batch.terminate;
                for result in &tool_results {
                    current_context
                        .messages
                        .push(AgentMessage::ToolResult(result.clone()));
                    new_messages.push(AgentMessage::ToolResult(result.clone()));
                }
            }

            (emit)(AgentEvent::TurnEnd {
                message: AgentMessage::Assistant(message.clone()),
                tool_results: tool_results.clone(),
            })
            .await;

            last_completed_turn = Some(PrepareNextTurnContext {
                message: message.clone(),
                tool_results: tool_results.clone(),
                context: current_context.clone(),
                new_messages: new_messages.clone(),
            });

            if let Some(hook) = &config.should_stop_after_turn {
                let stop = hook(ShouldStopAfterTurnContext {
                    message,
                    tool_results,
                    context: current_context.clone(),
                    new_messages: new_messages.clone(),
                })
                .await;
                if stop {
                    (emit)(AgentEvent::AgentEnd {
                        messages: new_messages.clone(),
                    })
                    .await;
                    return Ok(());
                }
            }

            // Steering is polled after every completed turn
            // (agent-loop.ts:263); the inner-loop condition decides whether
            // the queued messages start the next turn.
            pending_messages = poll_steering(&config).await;
        }

        // Agent would stop here. Check for follow-up messages
        // (agent-loop.ts:266-272); any message re-enters the inner loop.
        let follow_up_messages = poll_follow_up(&config).await;
        if !follow_up_messages.is_empty() {
            pending_messages = follow_up_messages;
            continue;
        }

        // No more messages, exit (agent-loop.ts:274-275).
        break;
    }

    (emit)(AgentEvent::AgentEnd {
        messages: new_messages.clone(),
    })
    .await;
    Ok(())
}

/// Upstream `(await config.getSteeringMessages?.()) || []`
/// (agent-loop.ts:174, 203, 263): poll the steering queue hook; an absent
/// hook polls to an empty injection.
async fn poll_steering(config: &AgentLoopConfig) -> Vec<AgentMessage> {
    match &config.get_steering_messages {
        Some(hook) => hook().await,
        None => Vec::new(),
    }
}

/// Upstream `(await config.getFollowUpMessages?.()) || []`
/// (agent-loop.ts:267): poll the follow-up queue hook at the stop point.
async fn poll_follow_up(config: &AgentLoopConfig) -> Vec<AgentMessage> {
    match &config.get_follow_up_messages {
        Some(hook) => hook().await,
        None => Vec::new(),
    }
}

/// Upstream `declareToolChanges` (agent-loop.ts:291-321): declare tool loadout
/// changes to the model as a system message before the first non-system
/// pending message.
fn declare_tool_changes(
    context: &AgentContext,
    pending_messages: Vec<AgentMessage>,
) -> Vec<AgentMessage> {
    let system_index = pending_messages
        .iter()
        .rposition(|message| message.role() == "system");
    let pending_system = system_index.and_then(|index| match &pending_messages[index] {
        AgentMessage::System(system) => Some(system.clone()),
        _ => None,
    });
    // Baseline: the pending messages with the pending system message's tool
    // fields treated as no-changes intent (agent-loop.ts:300-304).
    let baseline: Vec<AgentMessage> = match (&system_index, &pending_system) {
        (Some(index), Some(pending)) => {
            let mut baseline = pending_messages.clone();
            baseline[*index] = AgentMessage::System(with_tool_changes(
                pending.clone(),
                &ToolStateChanges::default(),
            ));
            baseline
        }
        _ => pending_messages.clone(),
    };

    let transcript: Vec<Message> = context
        .messages
        .iter()
        .chain(baseline.iter())
        .filter_map(|message| message.to_message())
        .collect();
    let changes = get_tool_state_changes(
        &get_current_tools(&transcript),
        &context
            .tools
            .iter()
            .map(|tool| tool.declaration())
            .collect::<Vec<_>>(),
    );
    let unchanged = changes.tools_added.is_empty() && changes.tools_removed.is_empty();

    if let (Some(index), Some(pending)) = (system_index, pending_system) {
        // Keep the caller's message when it already declares no tool changes.
        let declares_no_changes = pending
            .tools_added
            .as_ref()
            .is_none_or(|tools| tools.is_empty())
            && pending
                .tools_removed
                .as_ref()
                .is_none_or(|tools| tools.is_empty());
        if unchanged && declares_no_changes {
            return pending_messages;
        }
        let mut declared = baseline;
        declared[index] = AgentMessage::System(with_tool_changes(pending, &changes));
        return declared;
    }
    if unchanged {
        return pending_messages;
    }
    let update = with_tool_changes(
        SystemMessage {
            content: StringOrBlocks::Text(String::new()),
            sections: None,
            tools_added: None,
            tools_removed: None,
            timestamp: now_ms(),
        },
        &changes,
    );
    let insert_index = pending_messages
        .iter()
        .position(|message| message.role() != "system")
        .unwrap_or(pending_messages.len());
    let mut declared = pending_messages;
    declared.insert(insert_index, AgentMessage::System(update));
    declared
}

/// Upstream `withToolChanges` (agent-loop.ts:326-333): replace a system
/// message's tool fields with `changes`; empty lists omit the field.
fn with_tool_changes(mut message: SystemMessage, changes: &ToolStateChanges) -> SystemMessage {
    message.tools_added = (!changes.tools_added.is_empty()).then(|| changes.tools_added.clone());
    message.tools_removed =
        (!changes.tools_removed.is_empty()).then(|| changes.tools_removed.clone());
    message
}

/// Upstream `streamAssistantResponse` (agent-loop.ts:339-425): stream one
/// assistant response, mapping the provider's assistant events to
/// message_start/message_update/message_end agent events and appending the
/// final message to the context.
async fn stream_assistant_response(
    context: &mut AgentContext,
    config: &AgentLoopConfig,
    models: &Models,
    signal: Option<CancellationToken>,
    emit: &AgentEventSink,
) -> anyhow::Result<AssistantMessage> {
    // Apply the context transform if configured (AgentMessage[] ->
    // AgentMessage[]).
    let mut messages = context.messages.clone();
    if let Some(transform) = &config.transform_context {
        messages = transform(messages).await;
    }

    // Convert to LLM-compatible messages (AgentMessage[] -> Message[]).
    let llm_messages = (config.convert_to_llm)(messages).await;

    // Normalize before resolving the key or invoking a custom factory, just
    // like upstream. The typed transcript cannot carry prompt/tool shorthand.
    let request_context = Context {
        system_prompt: None,
        messages: llm_messages,
        tools: None,
    };
    let transcript = normalize_context(&request_context);
    let resolved_key = match &config.get_api_key {
        Some(resolve) => resolve(config.model.provider.clone()).await?,
        None => None,
    };
    let mut options = config.stream_options.clone();
    options.stream.api_key = resolved_key
        .filter(|key| !key.is_empty())
        .or(options.stream.api_key);
    options.stream.signal = signal;
    if let Some(session_id) = &config.session_id {
        options.stream.session_id = Some(session_id.clone());
    }
    if let Some(delay) = config.max_retry_delay_ms {
        options.stream.max_retry_delay_ms = Some(delay);
    }
    if let Some(budgets) = config.thinking_budgets {
        options.thinking_budgets = Some(budgets);
    }
    if let Some(level) = config.thinking_level {
        options.reasoning = match level {
            ThinkingLevel::Off => None,
            ThinkingLevel::Minimal => Some(RequestThinkingLevel::Minimal),
            ThinkingLevel::Low => Some(RequestThinkingLevel::Low),
            ThinkingLevel::Medium => Some(RequestThinkingLevel::Medium),
            ThinkingLevel::High => Some(RequestThinkingLevel::High),
            ThinkingLevel::Xhigh => Some(RequestThinkingLevel::Xhigh),
            ThinkingLevel::Max => Some(RequestThinkingLevel::Max),
        };
    }
    let mut rx = match &config.stream_fn {
        Some(stream) => stream(config.model.clone(), transcript, options).await?,
        None => models.stream_simple(
            &config.model,
            &request_context,
            Some(ModelsSimpleStreamOptions {
                simple: options,
                transform_headers: None,
            }),
        ),
    };

    // The live partial (upstream events carry `event.partial`; the port
    // reconstructs it from the event sequence).
    let mut partial = PartialAssistant::new();
    let mut added_partial = false;

    let final_message = loop {
        let Some(event) = rx.recv().await else {
            // Defensive: the StreamFn contract requires a terminal event;
            // a stream that closes without one settles as an error message
            // instead of hanging the loop.
            break synthesized_stream_error(&partial);
        };
        match &event {
            AssistantMessageEvent::Start { .. } => {
                let _ = partial.apply(&event);
                if let Some(snapshot) = partial.message().cloned() {
                    context
                        .messages
                        .push(AgentMessage::Assistant(snapshot.clone()));
                    added_partial = true;
                    (emit)(AgentEvent::MessageStart {
                        message: AgentMessage::Assistant(snapshot),
                    })
                    .await;
                }
            }
            AssistantMessageEvent::Done { message, .. }
            | AssistantMessageEvent::Error { error: message, .. } => {
                let _ = partial.apply(&event);
                break message.clone();
            }
            _ => {
                if added_partial {
                    let _ = partial.apply(&event);
                    if let Some(snapshot) = partial.message().cloned() {
                        if let Some(last) = context.messages.last_mut() {
                            *last = AgentMessage::Assistant(snapshot.clone());
                        }
                        (emit)(AgentEvent::MessageUpdate {
                            message: AgentMessage::Assistant(snapshot),
                            assistant_message_event: event,
                        })
                        .await;
                    }
                }
            }
        }
    };

    if added_partial {
        if let Some(last) = context.messages.last_mut() {
            *last = AgentMessage::Assistant(final_message.clone());
        }
    } else {
        context
            .messages
            .push(AgentMessage::Assistant(final_message.clone()));
    }
    if !added_partial {
        (emit)(AgentEvent::MessageStart {
            message: AgentMessage::Assistant(final_message.clone()),
        })
        .await;
    }
    (emit)(AgentEvent::MessageEnd {
        message: AgentMessage::Assistant(final_message.clone()),
    })
    .await;
    Ok(final_message)
}

/// The defensive settlement for a stream that closed without a terminal
/// `done`/`error` event (see the module docs): the reconstructed partial, or
/// a bare shell, marked with `stopReason: "error"`.
fn synthesized_stream_error(partial: &PartialAssistant) -> AssistantMessage {
    let mut message = partial.message().cloned().unwrap_or(AssistantMessage {
        content: Vec::new(),
        api: String::new(),
        provider: String::new(),
        model: String::new(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Error,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: now_ms(),
    });
    message.stop_reason = StopReason::Error;
    message.error_message = Some("Assistant stream ended without a terminal event".into());
    message
}

/// A tool call in an executed batch: the finalized outcome plus its
/// toolResult message (upstream `ExecutedToolCallBatch`, agent-loop.ts:481-484).
struct ExecutedToolCallBatch {
    messages: Vec<ToolResultMessage>,
    terminate: bool,
}

/// Upstream `failToolCallsFromTruncatedMessage` (agent-loop.ts:434-459): a
/// "length" stop means every tool call may carry truncated arguments; fail
/// them all instead of executing.
async fn fail_tool_calls_from_truncated_message(
    tool_calls: &[ToolCall],
    emit: &AgentEventSink,
) -> ExecutedToolCallBatch {
    let mut messages: Vec<ToolResultMessage> = Vec::new();
    for tool_call in tool_calls {
        (emit)(AgentEvent::ToolExecutionStart {
            tool_call_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            args: tool_call.arguments.clone(),
        })
        .await;
        let finalized = FinalizedToolCallOutcome {
            tool_call: tool_call.clone(),
            result: error_tool_result(format!(
                "Tool call \"{}\" was not executed: the response hit the output token limit, so its arguments may be truncated. Re-issue the tool call with complete arguments.",
                tool_call.name
            )),
            is_error: true,
        };
        emit_tool_execution_end(&finalized, emit).await;
        let tool_result_message = create_tool_result_message(&finalized);
        emit_tool_result_message(&tool_result_message, emit).await;
        messages.push(tool_result_message);
    }
    ExecutedToolCallBatch {
        messages,
        terminate: false,
    }
}

/// Upstream `executeToolCalls` (agent-loop.ts:464-479): dispatch to the
/// sequential or parallel executor.
async fn execute_tool_calls(
    context: &AgentContext,
    assistant_message: &AssistantMessage,
    config: &AgentLoopConfig,
    signal: Option<CancellationToken>,
    emit: &AgentEventSink,
) -> ExecutedToolCallBatch {
    let tool_calls: Vec<ToolCall> = assistant_message
        .content
        .iter()
        .filter_map(|block| match block {
            AssistantBlock::ToolCall(call) => Some(call.clone()),
            _ => None,
        })
        .collect();
    // Any sequential tool forces the whole batch sequential
    // (agent-loop.ts:472-474); a missing tool declaration does not.
    let has_sequential_tool_call = tool_calls.iter().any(|tool_call| {
        context
            .tools
            .iter()
            .find(|tool| tool.name == tool_call.name)
            .and_then(|tool| tool.execution_mode)
            == Some(ToolExecutionMode::Sequential)
    });
    if config.tool_execution == Some(ToolExecutionMode::Sequential) || has_sequential_tool_call {
        execute_tool_calls_sequential(context, assistant_message, tool_calls, config, signal, emit)
            .await
    } else {
        execute_tool_calls_parallel(context, assistant_message, tool_calls, config, signal, emit)
            .await
    }
}

/// Upstream `signal?.aborted` (agent-loop.ts:531, 569, 597, 691, 710, 576):
/// the port's [`CancellationToken`] equivalent.
fn signal_aborted(signal: Option<&CancellationToken>) -> bool {
    signal.is_some_and(CancellationToken::is_cancelled)
}

/// Upstream `executeToolCallsSequential` (agent-loop.ts:486-540).
async fn execute_tool_calls_sequential(
    context: &AgentContext,
    assistant_message: &AssistantMessage,
    tool_calls: Vec<ToolCall>,
    config: &AgentLoopConfig,
    signal: Option<CancellationToken>,
    emit: &AgentEventSink,
) -> ExecutedToolCallBatch {
    let mut finalized_calls: Vec<FinalizedToolCallOutcome> = Vec::new();
    let mut messages: Vec<ToolResultMessage> = Vec::new();

    for tool_call in tool_calls {
        (emit)(AgentEvent::ToolExecutionStart {
            tool_call_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            args: tool_call.arguments.clone(),
        })
        .await;

        let finalized = match prepare_tool_call(
            context,
            assistant_message,
            tool_call.clone(),
            config,
            signal.as_ref(),
        )
        .await
        {
            PreparedToolCall::Immediate { result, is_error } => FinalizedToolCallOutcome {
                tool_call,
                result,
                is_error,
            },
            PreparedToolCall::Prepared {
                tool_call,
                tool,
                args,
            } => {
                let executed = execute_prepared_tool_call(
                    &tool_call,
                    &tool,
                    args.clone(),
                    signal.clone(),
                    emit,
                )
                .await;
                finalize_executed_tool_call(
                    context,
                    assistant_message,
                    tool_call,
                    args,
                    executed,
                    config.after_tool_call.as_ref(),
                )
                .await
            }
        };

        emit_tool_execution_end(&finalized, emit).await;
        let tool_result_message = create_tool_result_message(&finalized);
        emit_tool_result_message(&tool_result_message, emit).await;
        finalized_calls.push(finalized);
        messages.push(tool_result_message);

        // Upstream breaks out of the loop when `signal.aborted`
        // (agent-loop.ts:531-533): remaining tool calls in the batch are
        // skipped entirely.
        if signal_aborted(signal.as_ref()) {
            break;
        }
    }

    ExecutedToolCallBatch {
        messages,
        terminate: should_terminate_tool_batch(&finalized_calls),
    }
}

/// Upstream `executeToolCallsParallel` (agent-loop.ts:542-616): preflight
/// sequentially, execute allowed tools concurrently, emit
/// `tool_execution_end` in completion order, then emit toolResult messages in
/// assistant source order.
async fn execute_tool_calls_parallel(
    context: &AgentContext,
    assistant_message: &AssistantMessage,
    tool_calls: Vec<ToolCall>,
    config: &AgentLoopConfig,
    signal: Option<CancellationToken>,
    emit: &AgentEventSink,
) -> ExecutedToolCallBatch {
    // Preflight loop (sequential); immediate outcomes finish in place, the
    // rest become concurrent entries. Nothing mutates the context while the
    // batch runs, so the hook payloads can share one snapshot.
    let context_snapshot = Arc::new(context.clone());
    let mut entries: Vec<ToolCallEntry> = Vec::new();
    for tool_call in tool_calls {
        (emit)(AgentEvent::ToolExecutionStart {
            tool_call_id: tool_call.id.clone(),
            tool_name: tool_call.name.clone(),
            args: tool_call.arguments.clone(),
        })
        .await;

        match prepare_tool_call(
            context,
            assistant_message,
            tool_call.clone(),
            config,
            signal.as_ref(),
        )
        .await
        {
            PreparedToolCall::Immediate { result, is_error } => {
                let finalized = FinalizedToolCallOutcome {
                    tool_call,
                    result,
                    is_error,
                };
                emit_tool_execution_end(&finalized, emit).await;
                entries.push(ToolCallEntry::Ready(finalized));
                // Upstream breaks out of the preflight loop when
                // `signal.aborted` (agent-loop.ts:569-571): later calls never
                // reach preflight.
                if signal_aborted(signal.as_ref()) {
                    break;
                }
            }
            PreparedToolCall::Prepared {
                tool_call,
                tool,
                args,
            } => {
                entries.push(ToolCallEntry::Pending {
                    tool_call,
                    tool,
                    args,
                });
                // Same preflight break after a pending entry
                // (agent-loop.ts:597-599).
                if signal_aborted(signal.as_ref()) {
                    break;
                }
            }
        }
    }

    // Execute concurrently; each entry emits its own tool_execution_end as it
    // finishes (completion order, agent-loop.ts:602-604).
    let executions = entries.into_iter().map(|entry| {
        let emit = Arc::clone(emit);
        let after_hook = config.after_tool_call.clone();
        let snapshot = Arc::clone(&context_snapshot);
        let assistant_message = assistant_message.clone();
        let signal = signal.clone();
        async move {
            match entry {
                ToolCallEntry::Ready(finalized) => finalized,
                ToolCallEntry::Pending {
                    tool_call,
                    tool,
                    args,
                } => {
                    // Upstream checks `signal.aborted` at execution time and
                    // fails the call without running the tool
                    // (agent-loop.ts:576-584).
                    if signal_aborted(signal.as_ref()) {
                        let finalized = FinalizedToolCallOutcome {
                            tool_call,
                            result: error_tool_result("Operation aborted"),
                            is_error: true,
                        };
                        emit_tool_execution_end(&finalized, &emit).await;
                        return finalized;
                    }
                    let executed =
                        execute_prepared_tool_call(&tool_call, &tool, args.clone(), signal, &emit)
                            .await;
                    let finalized = finalize_executed_tool_call(
                        &snapshot,
                        &assistant_message,
                        tool_call,
                        args,
                        executed,
                        after_hook.as_ref(),
                    )
                    .await;
                    emit_tool_execution_end(&finalized, &emit).await;
                    finalized
                }
            }
        }
    });
    let ordered_finalized_calls = join_all(executions).await;

    // ToolResult message artifacts emit later, in assistant source order
    // (agent-loop.ts:605-610).
    let mut messages: Vec<ToolResultMessage> = Vec::new();
    for finalized in &ordered_finalized_calls {
        let tool_result_message = create_tool_result_message(finalized);
        emit_tool_result_message(&tool_result_message, emit).await;
        messages.push(tool_result_message);
    }

    ExecutedToolCallBatch {
        messages,
        terminate: should_terminate_tool_batch(&ordered_finalized_calls),
    }
}

/// One preflighted entry of a parallel batch (upstream
/// `FinalizedToolCallEntry`, agent-loop.ts:642: an immediate outcome or a
/// pending execution).
enum ToolCallEntry {
    Ready(FinalizedToolCallOutcome),
    Pending {
        tool_call: ToolCall,
        tool: Arc<AgentTool>,
        args: serde_json::Value,
    },
}

/// Upstream `FinalizedToolCallOutcome` (agent-loop.ts:636-640).
struct FinalizedToolCallOutcome {
    tool_call: ToolCall,
    result: AgentToolResult,
    is_error: bool,
}

/// Upstream `PreparedToolCall`/`ImmediateToolCallOutcome`
/// (agent-loop.ts:618-629): a validated call ready to execute, or an
/// immediate outcome that skips execution.
enum PreparedToolCall {
    Prepared {
        tool_call: ToolCall,
        tool: Arc<AgentTool>,
        args: serde_json::Value,
    },
    Immediate {
        result: AgentToolResult,
        is_error: bool,
    },
}

/// Upstream `ExecutedToolCallOutcome` (agent-loop.ts:631-634).
struct ExecutedToolCallOutcome {
    result: AgentToolResult,
    is_error: bool,
}

/// Upstream `shouldTerminateToolBatch` (agent-loop.ts:644-646): terminate only
/// when every finalized result in the batch sets it.
fn should_terminate_tool_batch(finalized_calls: &[FinalizedToolCallOutcome]) -> bool {
    !finalized_calls.is_empty()
        && finalized_calls
            .iter()
            .all(|finalized| finalized.result.terminate == Some(true))
}

/// Upstream `prepareToolCall` (agent-loop.ts:662-730): resolve the tool,
/// apply `prepareArguments`, validate, then run the `beforeToolCall` hook.
async fn prepare_tool_call(
    context: &AgentContext,
    assistant_message: &AssistantMessage,
    tool_call: ToolCall,
    config: &AgentLoopConfig,
    signal: Option<&CancellationToken>,
) -> PreparedToolCall {
    let Some(tool) = context
        .tools
        .iter()
        .find(|tool| tool.name == tool_call.name)
        .cloned()
    else {
        return PreparedToolCall::Immediate {
            result: error_tool_result(format!("Tool {} not found", tool_call.name)),
            is_error: true,
        };
    };

    // Compatibility shim, then schema validation (upstream wraps shim,
    // validation, and hook in one try/catch; the port's infallible hooks
    // collapse it to validation errors alone, agent-loop.ts:678-680, 723-729).
    let prepared_arguments = match &tool.prepare_arguments {
        Some(prepare) => prepare(tool_call.arguments.clone()),
        None => tool_call.arguments.clone(),
    };
    let validated_arguments = match validate_tool_arguments(
        &tool.declaration(),
        &ToolCall {
            arguments: prepared_arguments,
            ..tool_call.clone()
        },
    ) {
        Ok(arguments) => arguments,
        Err(message) => {
            return PreparedToolCall::Immediate {
                result: error_tool_result(message),
                is_error: true,
            };
        }
    };

    if let Some(hook) = &config.before_tool_call {
        let outcome = hook(BeforeToolCallContext {
            assistant_message: assistant_message.clone(),
            tool_call: tool_call.clone(),
            args: validated_arguments.clone(),
            context: context.clone(),
        })
        .await;
        // Upstream re-checks `signal.aborted` after the hook
        // (agent-loop.ts:691-697): an aborted call fails without running the
        // tool, ahead of any block decision.
        if signal_aborted(signal) {
            return PreparedToolCall::Immediate {
                result: error_tool_result("Operation aborted"),
                is_error: true,
            };
        }
        if let Some(result) = &outcome.result {
            if result.block == Some(true) {
                let mut result_override = error_tool_result(
                    result
                        .reason
                        .clone()
                        .unwrap_or_else(|| "Tool execution was blocked".to_string()),
                );
                if result.terminate == Some(true) {
                    result_override.terminate = Some(true);
                }
                return PreparedToolCall::Immediate {
                    result: result_override,
                    is_error: true,
                };
            }
        }
        if let Some(args) = outcome.args {
            return PreparedToolCall::Prepared {
                tool_call,
                tool,
                args,
            };
        }
    }
    // Upstream checks `signal.aborted` before returning the prepared call
    // (agent-loop.ts:710-716).
    if signal_aborted(signal) {
        return PreparedToolCall::Immediate {
            result: error_tool_result("Operation aborted"),
            is_error: true,
        };
    }
    PreparedToolCall::Prepared {
        tool_call,
        tool,
        args: validated_arguments,
    }
}

/// Upstream `executePreparedToolCall` (agent-loop.ts:732-773): run the tool,
/// collecting streamed updates as `tool_execution_update` emissions; the
/// emissions run after `execute` resolves and later `onUpdate` calls are
/// ignored (`acceptingUpdates` guard), so a tool cannot emit after it
/// settles. Failures become error results.
async fn execute_prepared_tool_call(
    tool_call: &ToolCall,
    tool: &AgentTool,
    args: serde_json::Value,
    signal: Option<CancellationToken>,
    emit: &AgentEventSink,
) -> ExecutedToolCallOutcome {
    let accepting_updates = Arc::new(AtomicBool::new(true));
    let update_events: Arc<Mutex<Vec<BoxFuture<'static, ()>>>> = Arc::new(Mutex::new(Vec::new()));
    let on_update: Arc<AgentToolUpdateCallback> = {
        let (accepting_updates, update_events) =
            (Arc::clone(&accepting_updates), Arc::clone(&update_events));
        let update_sink = Arc::clone(emit);
        let update_tool_call = tool_call.clone();
        Arc::new(move |partial_result: &AgentToolResult| {
            if !accepting_updates.load(Ordering::SeqCst) {
                return;
            }
            let update_sink = Arc::clone(&update_sink);
            let event = AgentEvent::ToolExecutionUpdate {
                tool_call_id: update_tool_call.id.clone(),
                tool_name: update_tool_call.name.clone(),
                args: update_tool_call.arguments.clone(),
                partial_result: serde_json::to_value(partial_result)
                    .expect("tool result serializes"),
            };
            update_events.lock().unwrap().push(update_sink(event));
        })
    };
    let executed = match (tool.execute)(tool_call.id.clone(), args, signal, Some(on_update)).await {
        Ok(result) => ExecutedToolCallOutcome {
            result,
            is_error: false,
        },
        Err(error) => ExecutedToolCallOutcome {
            result: error_tool_result(error),
            is_error: true,
        },
    };
    // Upstream: `acceptingUpdates = false; await Promise.all(updateEvents)`.
    accepting_updates.store(false, Ordering::SeqCst);
    let pending: Vec<BoxFuture<'static, ()>> = std::mem::take(&mut *update_events.lock().unwrap());
    for event in pending {
        event.await;
    }
    executed
}

/// Upstream `finalizeExecutedToolCall` (agent-loop.ts:775-820): apply the
/// `afterToolCall` field-by-field override.
async fn finalize_executed_tool_call(
    context: &AgentContext,
    assistant_message: &AssistantMessage,
    tool_call: ToolCall,
    args: serde_json::Value,
    executed: ExecutedToolCallOutcome,
    after_hook: Option<&Arc<AfterToolCallHook>>,
) -> FinalizedToolCallOutcome {
    let mut result = executed.result;
    let mut is_error = executed.is_error;

    if let Some(hook) = after_hook {
        let after = hook(AfterToolCallContext {
            assistant_message: assistant_message.clone(),
            tool_call: tool_call.clone(),
            args,
            result: result.clone(),
            is_error,
            context: context.clone(),
        })
        .await;
        // Field-by-field merge; a Some field replaces the executed value in
        // full (agent-loop.ts:799-808).
        if let Some(after) = after {
            if let Some(content) = after.content {
                result.content = content;
            }
            if let Some(details) = after.details {
                result.details = Some(details);
            }
            if let Some(usage) = after.usage {
                result.usage = Some(usage);
            }
            if let Some(terminate) = after.terminate {
                result.terminate = Some(terminate);
            }
            if let Some(hook_is_error) = after.is_error {
                is_error = hook_is_error;
            }
        }
    }

    FinalizedToolCallOutcome {
        tool_call,
        result,
        is_error,
    }
}

/// Upstream `createErrorToolResult` (agent-loop.ts:822-827).
fn error_tool_result(message: impl std::fmt::Display) -> AgentToolResult {
    AgentToolResult {
        content: vec![TextOrImageBlock::Text(TextContent {
            text: message.to_string(),
            text_signature: None,
        })],
        details: Some(serde_json::Value::Object(serde_json::Map::new())),
        usage: None,
        terminate: None,
    }
}

/// Upstream `emitToolExecutionEnd` (agent-loop.ts:829-837).
async fn emit_tool_execution_end(finalized: &FinalizedToolCallOutcome, emit: &AgentEventSink) {
    (emit)(AgentEvent::ToolExecutionEnd {
        tool_call_id: finalized.tool_call.id.clone(),
        tool_name: finalized.tool_call.name.clone(),
        result: serde_json::to_value(&finalized.result).expect("tool result serializes"),
        is_error: finalized.is_error,
    })
    .await;
}

/// Upstream `createToolResultMessage` (agent-loop.ts:839-852).
fn create_tool_result_message(finalized: &FinalizedToolCallOutcome) -> ToolResultMessage {
    ToolResultMessage {
        tool_call_id: finalized.tool_call.id.clone(),
        tool_name: finalized.tool_call.name.clone(),
        content: finalized.result.content.clone(),
        details: finalized.result.details.clone(),
        usage: finalized.result.usage,
        is_error: finalized.is_error,
        timestamp: now_ms(),
    }
}

/// Upstream `emitToolResultMessage` (agent-loop.ts:854-857).
async fn emit_tool_result_message(message: &ToolResultMessage, emit: &AgentEventSink) {
    let agent_message = AgentMessage::ToolResult(message.clone());
    (emit)(AgentEvent::MessageStart {
        message: agent_message.clone(),
    })
    .await;
    (emit)(AgentEvent::MessageEnd {
        message: agent_message,
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core::types::make_tool;
    use crate::agent_core::CustomAgentMessage;
    use crate::ai::api::anthropic::AnthropicMessages;
    use crate::ai::api::openai_completions::OpenAiCompletions;
    use crate::ai::api::openai_responses::OpenAiResponses;
    use crate::ai::auth::types::{
        ApiKeyAuth, ApiKeyAuthInput, AuthError, AuthResult, ModelAuth, ProviderAuth,
    };
    use crate::ai::models::faux::FauxTokenSize;
    use crate::ai::models::provider::{create_provider, ApiImpls, CreateProviderOptions};
    use crate::ai::models::{
        create_models, faux_assistant_message, faux_provider, faux_tool_call, CreateModelsOptions,
        FauxFactoryArgs, FauxMessageOptions, FauxProviderHandle, FauxProviderOptions,
        FauxResponseStep, FauxToolCallOptions,
    };
    use crate::ai::transcript::content_text;
    use crate::ai::types::message::UserMessage;
    use crate::ai::types::primitives::{ModelCost, UsageCost};
    use crate::ai::types::Model;
    use crate::ai::types::ModelInput;
    use crate::ai::ApiImpl;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::sync::Notify;

    // ---- fixtures (agent-loop.test.ts:29-84) ----

    fn faux_models() -> (Arc<Models>, FauxProviderHandle, crate::ai::types::Model) {
        let faux = faux_provider(FauxProviderOptions::default());
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(faux.provider.clone());
        let model = faux.get_model(None).expect("faux default model");
        (Arc::new(models), faux, model)
    }

    fn user_message(text: &str) -> AgentMessage {
        AgentMessage::User(UserMessage {
            content: StringOrBlocks::Text(text.to_string()),
            timestamp: now_ms(),
        })
    }

    /// Upstream `identityConverter` (agent-loop.test.ts:80-84): pass through
    /// the standard roles.
    fn identity_convert(messages: Vec<AgentMessage>) -> BoxFuture<'static, Vec<Message>> {
        Box::pin(async move {
            messages
                .iter()
                .filter_map(|message| message.to_message())
                .collect()
        })
    }

    fn identity_config(model: crate::ai::types::Model) -> AgentLoopConfig {
        AgentLoopConfig::new(model, Arc::new(identity_convert))
    }

    fn text_response(text: &str) -> FauxResponseStep {
        faux_assistant_message(text, FauxMessageOptions::default()).into()
    }

    fn tool_call_response(
        calls: Vec<(&str, &str, serde_json::Value)>,
        stop_reason: StopReason,
    ) -> FauxResponseStep {
        let blocks: Vec<AssistantBlock> = calls
            .into_iter()
            .map(|(id, name, arguments)| {
                faux_tool_call(
                    name,
                    arguments,
                    FauxToolCallOptions {
                        id: Some(id.to_string()),
                    },
                )
            })
            .collect();
        faux_assistant_message(
            blocks,
            FauxMessageOptions {
                stop_reason: Some(stop_reason),
                ..FauxMessageOptions::default()
            },
        )
        .into()
    }

    fn text_block(text: &str) -> TextOrImageBlock {
        TextOrImageBlock::Text(TextContent {
            text: text.to_string(),
            text_signature: None,
        })
    }

    fn event_name(event: &AgentEvent) -> &'static str {
        match event {
            AgentEvent::Preserved(_) => event_name(event.kind()),
            AgentEvent::AgentStart => "agent_start",
            AgentEvent::AgentEnd { .. } => "agent_end",
            AgentEvent::TurnStart => "turn_start",
            AgentEvent::TurnEnd { .. } => "turn_end",
            AgentEvent::MessageStart { .. } => "message_start",
            AgentEvent::MessageUpdate { .. } => "message_update",
            AgentEvent::MessageEnd { .. } => "message_end",
            AgentEvent::ToolExecutionStart { .. } => "tool_execution_start",
            AgentEvent::ToolExecutionUpdate { .. } => "tool_execution_update",
            AgentEvent::ToolExecutionEnd { .. } => "tool_execution_end",
        }
    }

    fn event_names(events: &[AgentEvent]) -> Vec<&'static str> {
        events.iter().map(event_name).collect()
    }

    /// Lifecycle sequence with runs of `message_update` collapsed: the faux
    /// provider streams the full per-block choreography (start, deltas, end),
    /// so streamed assistant turns carry a chunk-count-dependent number of
    /// `message_update` events. Upstream's oracle used a mock that emitted
    /// only `done` and could pin the bare sequence.
    fn canonical_event_names(events: &[AgentEvent]) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = Vec::new();
        for event in events {
            match event_name(event) {
                "message_update" => {
                    if names.last() != Some(&"message_update*") {
                        names.push("message_update*");
                    }
                }
                name => names.push(name),
            }
        }
        names
    }

    async fn collect_events(mut rx: mpsc::UnboundedReceiver<AgentEvent>) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    }

    fn role_names(messages: &[AgentMessage]) -> Vec<&str> {
        messages.iter().map(|message| message.role()).collect()
    }

    /// An echo tool recording every executed `value` argument.
    fn echo_tool(executed: Arc<Mutex<Vec<serde_json::Value>>>) -> AgentTool {
        AgentTool {
            name: "echo".into(),
            label: "Echo".into(),
            description: "Echo tool".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"value": {"type": "string"}},
                "required": ["value"]
            }),
            execute: Arc::new(
                move |_id: String,
                      params: serde_json::Value,
                      _signal: Option<CancellationToken>,
                      _on_update: Option<Arc<AgentToolUpdateCallback>>| {
                    let executed = executed.clone();
                    Box::pin(async move {
                        executed.lock().unwrap().push(params["value"].clone());
                        let value = params["value"].as_str().unwrap_or_default();
                        Ok(AgentToolResult {
                            content: vec![text_block(&format!("echoed: {value}"))],
                            details: Some(serde_json::json!({"value": params["value"]})),
                            ..AgentToolResult::default()
                        })
                    })
                },
            ),
            constrained_sampling: None,
            prepare_arguments: None,
            replay: None,
            execution_mode: None,
        }
    }

    /// Upstream's blocking tools (agent-loop.test.ts:629-648, 828-847, 996-1015):
    /// records `{value}` on execute; when `value == wait_value`, waits for the
    /// notify and then marks `first_resolved`; when `value == "second"` runs
    /// before `first_resolved`, marks `parallel_observed`.
    fn blocking_tool(
        name: &str,
        wait_value: &str,
        notify: Arc<Notify>,
        first_resolved: Arc<AtomicBool>,
        parallel_observed: Arc<AtomicBool>,
        executed: Arc<Mutex<Vec<String>>>,
        mode: Option<ToolExecutionMode>,
    ) -> AgentTool {
        let name = name.to_string();
        let wait_value = wait_value.to_string();
        AgentTool {
            name: name.clone(),
            label: name.clone(),
            description: format!("{name} tool"),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"value": {"type": "string"}},
                "required": ["value"]
            }),
            execute: Arc::new(
                move |_id: String,
                      params: serde_json::Value,
                      _signal: Option<CancellationToken>,
                      _on_update: Option<Arc<AgentToolUpdateCallback>>| {
                    let (name, wait_value) = (name.clone(), wait_value.clone());
                    let (notify, first_resolved, parallel_observed, executed) = (
                        notify.clone(),
                        first_resolved.clone(),
                        parallel_observed.clone(),
                        executed.clone(),
                    );
                    Box::pin(async move {
                        let value = params["value"].as_str().unwrap_or_default().to_string();
                        executed.lock().unwrap().push(value.clone());
                        if value == wait_value {
                            notify.notified().await;
                            first_resolved.store(true, Ordering::SeqCst);
                        }
                        if value == "second" && !first_resolved.load(Ordering::SeqCst) {
                            parallel_observed.store(true, Ordering::SeqCst);
                        }
                        Ok(AgentToolResult {
                            content: vec![text_block(&format!("{name}: {value}"))],
                            details: Some(serde_json::json!({"value": value})),
                            ..AgentToolResult::default()
                        })
                    })
                },
            ),
            constrained_sampling: None,
            prepare_arguments: None,
            replay: None,
            execution_mode: mode,
        }
    }

    /// Upstream's ordered slow/fast tools (agent-loop.test.ts:909-941):
    /// records `{name}:{value}` on execute; waits for the notify when
    /// `wait_value` matches.
    fn recording_tool(
        name: &str,
        wait_value: Option<&str>,
        notify: Option<Arc<Notify>>,
        executed: Arc<Mutex<Vec<String>>>,
        mode: Option<ToolExecutionMode>,
    ) -> AgentTool {
        let name = name.to_string();
        let wait_value = wait_value.map(str::to_string);
        AgentTool {
            name: name.clone(),
            label: name.clone(),
            description: format!("{name} tool"),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"value": {"type": "string"}},
                "required": ["value"]
            }),
            execute: Arc::new(
                move |_id: String,
                      params: serde_json::Value,
                      _signal: Option<CancellationToken>,
                      _on_update: Option<Arc<AgentToolUpdateCallback>>| {
                    let (name, wait_value) = (name.clone(), wait_value.clone());
                    let (notify, executed) = (notify.clone(), executed.clone());
                    Box::pin(async move {
                        let value = params["value"].as_str().unwrap_or_default().to_string();
                        executed.lock().unwrap().push(format!("{name}:{value}"));
                        if wait_value.as_deref() == Some(value.as_str()) {
                            if let Some(notify) = notify.as_ref() {
                                notify.notified().await;
                            }
                        }
                        Ok(AgentToolResult {
                            content: vec![text_block(&format!("{name}: {value}"))],
                            details: Some(serde_json::json!({"value": value})),
                            ..AgentToolResult::default()
                        })
                    })
                },
            ),
            constrained_sampling: None,
            prepare_arguments: None,
            replay: None,
            execution_mode: mode,
        }
    }

    fn spawn_releaser(notify: &Arc<Notify>, millis: u64) {
        let notify = Arc::clone(notify);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(millis)).await;
            notify.notify_one();
        });
    }

    fn call_count(faux: &FauxProviderHandle) -> u64 {
        faux.state().lock().unwrap().call_count
    }

    // ---- agentLoop with AgentMessage (oracle block, agent-loop.test.ts:120) ----

    /// Oracle "should emit events with AgentMessage types".
    #[tokio::test]
    async fn emits_events_with_agent_message_types() {
        let (models, faux, model) = faux_models();
        faux.set_responses(vec![text_response("Hi there!")]);
        let (rx, handle) = agent_loop(
            vec![user_message("Hello")],
            AgentContext::default(),
            identity_config(model),
            models,
            None,
        );

        let messages = handle.await.unwrap().unwrap();
        let events = collect_events(rx).await;

        assert_eq!(messages.len(), 2);
        assert_eq!(role_names(&messages), ["user", "assistant"]);

        let names = event_names(&events);
        for expected in [
            "agent_start",
            "turn_start",
            "message_start",
            "message_end",
            "turn_end",
            "agent_end",
        ] {
            assert!(names.contains(&expected), "missing {expected}: {names:?}");
        }
        assert_eq!(names.first().copied(), Some("agent_start"));
        assert_eq!(names.last().copied(), Some("agent_end"));
    }

    /// Oracle "should build provider context exclusively from transcript
    /// messages": the provider receives only the normalized transcript.
    #[tokio::test]
    async fn builds_provider_context_exclusively_from_transcript_messages() {
        let (models, faux, model) = faux_models();
        let initial_system = AgentMessage::System(SystemMessage {
            content: StringOrBlocks::Text("Transcript prompt".into()),
            sections: None,
            tools_added: Some(Vec::new()),
            tools_removed: None,
            timestamp: 1,
        });
        #[derive(Clone)]
        struct Observed {
            len: usize,
            first_is_system: bool,
            first_text: String,
            first_timestamp: i64,
            second_is_user_hello: bool,
        }
        let observed = Arc::new(Mutex::new(None::<Observed>));
        let writer = observed.clone();
        faux.set_responses(vec![FauxResponseStep::Factory(Arc::new(
            move |args: FauxFactoryArgs| {
                let writer = writer.clone();
                Box::pin(async move {
                    let messages = args.context.messages();
                    let mut observation = Observed {
                        len: messages.len(),
                        first_is_system: false,
                        first_text: String::new(),
                        first_timestamp: 0,
                        second_is_user_hello: false,
                    };
                    if let Some(Message::System(system)) = messages.first() {
                        observation.first_is_system = true;
                        observation.first_text = content_text(&system.content);
                        observation.first_timestamp = system.timestamp;
                    }
                    if let Some(Message::User(user)) = messages.get(1) {
                        observation.second_is_user_hello = content_text(&user.content) == "Hello";
                    }
                    *writer.lock().unwrap() = Some(observation);
                    Ok(faux_assistant_message(
                        "done",
                        FauxMessageOptions::default(),
                    ))
                })
            },
        ))]);

        let (_rx, handle) = agent_loop(
            vec![initial_system, user_message("Hello")],
            AgentContext::default(),
            identity_config(model),
            models,
            None,
        );
        handle.await.unwrap().unwrap();

        let observation = observed.lock().unwrap().clone().expect("factory ran");
        assert_eq!(observation.len, 2);
        assert!(observation.first_is_system);
        assert_eq!(observation.first_text, "Transcript prompt");
        assert_eq!(observation.first_timestamp, 1);
        assert!(observation.second_is_user_hello);
    }

    /// Oracle "should handle custom message types via convertToLlm".
    #[tokio::test]
    async fn handles_custom_message_types_via_convert_to_llm() {
        let (models, faux, model) = faux_models();
        let notification = AgentMessage::Custom(CustomAgentMessage {
            role: "notification".into(),
            data: {
                let mut data = serde_json::Map::new();
                data.insert("text".into(), "This is a notification".into());
                data.insert("timestamp".into(), serde_json::json!(now_ms()));
                data
            },
        });
        let converted_seen = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let writer = converted_seen.clone();
        let config = AgentLoopConfig::new(
            model,
            Arc::new(move |messages| {
                let writer = writer.clone();
                Box::pin(async move {
                    let converted: Vec<Message> = messages
                        .iter()
                        .filter(|message| message.role() != "notification")
                        .filter(|message| {
                            matches!(message.role(), "user" | "assistant" | "toolResult")
                        })
                        .filter_map(|message| message.to_message())
                        .collect();
                    *writer.lock().unwrap() = converted
                        .iter()
                        .map(|message| match message {
                            Message::System(_) => "system",
                            Message::User(_) => "user",
                            Message::Assistant(_) => "assistant",
                            Message::ToolResult(_) => "toolResult",
                        })
                        .collect();
                    converted
                })
            }),
        );
        faux.set_responses(vec![text_response("Response")]);

        let (rx, handle) = agent_loop(
            vec![user_message("Hello")],
            AgentContext {
                messages: vec![notification],
                tools: Vec::new(),
            },
            config,
            models,
            None,
        );
        let _messages = handle.await.unwrap().unwrap();
        collect_events(rx).await;

        assert_eq!(*converted_seen.lock().unwrap(), ["user"]);
    }

    /// Oracle "should apply transformContext before convertToLlm".
    #[tokio::test]
    async fn applies_transform_context_before_convert_to_llm() {
        let (models, faux, model) = faux_models();
        let context = AgentContext {
            messages: vec![
                user_message("old message 1"),
                AgentMessage::Assistant(faux_assistant_message(
                    "old response 1",
                    FauxMessageOptions::default(),
                )),
                user_message("old message 2"),
                AgentMessage::Assistant(faux_assistant_message(
                    "old response 2",
                    FauxMessageOptions::default(),
                )),
            ],
            tools: Vec::new(),
        };
        let transformed_seen = Arc::new(Mutex::new(0usize));
        let converted_seen = Arc::new(Mutex::new(0usize));
        let transform_writer = transformed_seen.clone();
        let convert_writer = converted_seen.clone();
        let mut config = AgentLoopConfig::new(
            model,
            Arc::new(move |messages: Vec<AgentMessage>| {
                let writer = convert_writer.clone();
                Box::pin(async move {
                    *writer.lock().unwrap() = messages.len();
                    messages.iter().filter_map(|m| m.to_message()).collect()
                })
            }),
        );
        config.transform_context = Some(Arc::new(move |messages: Vec<AgentMessage>| {
            let writer = transform_writer.clone();
            Box::pin(async move {
                let pruned = messages[messages.len() - 2..].to_vec();
                *writer.lock().unwrap() = pruned.len();
                pruned
            })
        }));
        faux.set_responses(vec![text_response("Response")]);

        let (rx, handle) = agent_loop(
            vec![user_message("new message")],
            context,
            config,
            models,
            None,
        );
        let _messages = handle.await.unwrap().unwrap();
        collect_events(rx).await;

        assert_eq!(*transformed_seen.lock().unwrap(), 2);
        assert_eq!(*converted_seen.lock().unwrap(), 2);
    }

    /// Oracle "should handle tool calls and results": the tool executes, the
    /// afterToolCall hook observes the executed usage and patches the tool
    /// result's usage.
    #[tokio::test]
    async fn handles_tool_calls_and_results_with_after_tool_call_usage_patch() {
        let (models, faux, model) = faux_models();
        let executed = Arc::new(Mutex::new(Vec::<String>::new()));
        let tool_usage = Usage {
            input: 1,
            output: 2,
            cache_read: 3,
            cache_write: 4,
            cache_write_1h: None,
            reasoning: None,
            total_tokens: 10,
            cost: UsageCost {
                input: 0.1,
                output: 0.2,
                cache_read: 0.3,
                cache_write: 0.4,
                total: 1.0,
            },
        };
        let patched_tool_usage = Usage {
            input: 5,
            output: 6,
            cache_read: 7,
            cache_write: 8,
            cache_write_1h: None,
            reasoning: None,
            total_tokens: 26,
            cost: UsageCost {
                input: 0.5,
                output: 0.6,
                cache_read: 0.7,
                cache_write: 0.8,
                total: 2.6,
            },
        };
        let tool_usage_copy = tool_usage;
        let executed_writer = executed.clone();
        let tool = AgentTool {
            name: "echo".into(),
            label: "Echo".into(),
            description: "Echo tool".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"value": {"type": "string"}},
                "required": ["value"]
            }),
            execute: Arc::new(
                move |_id: String,
                      params: serde_json::Value,
                      _signal: Option<CancellationToken>,
                      _on_update: Option<Arc<AgentToolUpdateCallback>>| {
                    let (executed, tool_usage) = (executed_writer.clone(), tool_usage_copy);
                    Box::pin(async move {
                        let value = params["value"].as_str().unwrap_or_default().to_string();
                        executed.lock().unwrap().push(value.clone());
                        Ok(AgentToolResult {
                            content: vec![text_block(&format!("echoed: {value}"))],
                            details: Some(serde_json::json!({"value": value})),
                            usage: Some(tool_usage),
                            terminate: None,
                        })
                    })
                },
            ),
            constrained_sampling: None,
            prepare_arguments: None,
            replay: None,
            execution_mode: None,
        };
        let observed = Arc::new(Mutex::new(None::<Usage>));
        let observed_writer = observed.clone();
        let patched_copy = patched_tool_usage;
        let mut config = identity_config(model);
        config.after_tool_call = Some(Arc::new(move |hook: AfterToolCallContext| {
            let (observed_writer, patched) = (observed_writer.clone(), patched_copy);
            Box::pin(async move {
                *observed_writer.lock().unwrap() = hook.result.usage;
                Some(AfterToolCallResult {
                    usage: Some(patched),
                    ..AfterToolCallResult::default()
                })
            })
        }));
        faux.set_responses(vec![
            tool_call_response(
                vec![("tool-1", "echo", serde_json::json!({"value": "hello"}))],
                StopReason::ToolUse,
            ),
            text_response("done"),
        ]);

        let (rx, handle) = agent_loop(
            vec![user_message("echo something")],
            AgentContext {
                messages: Vec::new(),
                tools: vec![Arc::new(tool)],
            },
            config,
            models,
            None,
        );
        let messages = handle.await.unwrap().unwrap();
        let events = collect_events(rx).await;

        assert_eq!(*executed.lock().unwrap(), ["hello"]);
        let tool_end = events.iter().find_map(|event| match event {
            AgentEvent::ToolExecutionEnd {
                is_error, result, ..
            } => Some((*is_error, result.clone())),
            _ => None,
        });
        let (is_error, result) = tool_end.expect("tool_execution_end event");
        assert!(!is_error);
        assert_eq!(
            result["content"][0]["text"],
            serde_json::json!("echoed: hello")
        );
        assert_eq!(*observed.lock().unwrap(), Some(tool_usage));
        let tool_result_usage = messages
            .iter()
            .find_map(|message| match message {
                AgentMessage::ToolResult(tool_result) => Some(tool_result.usage),
                _ => None,
            })
            .flatten();
        assert_eq!(tool_result_usage, Some(patched_tool_usage));
    }

    /// Oracle "should not execute tool calls from a length-truncated assistant
    /// message": every call fails with the truncation error and the loop
    /// continues so the model can re-issue.
    #[tokio::test]
    async fn does_not_execute_tool_calls_from_a_length_truncated_message() {
        let (models, faux, model) = faux_models();
        let executed = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        faux.set_responses(vec![
            tool_call_response(
                vec![("tool-1", "echo", serde_json::json!({"value": "hel"}))],
                StopReason::Length,
            ),
            text_response("done"),
        ]);

        let (rx, handle) = agent_loop(
            vec![user_message("echo something")],
            AgentContext {
                messages: Vec::new(),
                tools: vec![Arc::new(echo_tool(executed.clone()))],
            },
            identity_config(model),
            models,
            None,
        );
        let messages = handle.await.unwrap().unwrap();
        let events = collect_events(rx).await;

        assert!(executed.lock().unwrap().is_empty());
        let tool_end = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::ToolExecutionEnd { result, .. } => Some(result.clone()),
                _ => None,
            })
            .expect("tool_execution_end event");
        assert_eq!(
            tool_end["content"][0]["text"].as_str().unwrap_or_default(),
            "Tool call \"echo\" was not executed: the response hit the output token limit, so its arguments may be truncated. Re-issue the tool call with complete arguments."
        );
        let is_error = events.iter().find_map(|event| match event {
            AgentEvent::ToolExecutionEnd { is_error, .. } => Some(*is_error),
            _ => None,
        });
        assert_eq!(is_error, Some(true));
        assert_eq!(call_count(&faux), 2);
        assert_eq!(messages.last().map(role_names_one), Some("assistant"));
    }

    fn role_names_one(message: &AgentMessage) -> &str {
        message.role()
    }

    /// Oracle "should execute mutated beforeToolCall args without
    /// revalidation": the hook's replacement args reach the tool verbatim.
    #[tokio::test]
    async fn executes_mutated_before_tool_call_args_without_revalidation() {
        let (models, faux, model) = faux_models();
        let executed = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let mut config = identity_config(model);
        config.before_tool_call = Some(Arc::new(|_hook: BeforeToolCallContext| {
            Box::pin(async move {
                BeforeToolCallOutcome {
                    args: Some(serde_json::json!({"value": 123})),
                    result: None,
                }
            })
        }));
        faux.set_responses(vec![
            tool_call_response(
                vec![("tool-1", "echo", serde_json::json!({"value": "hello"}))],
                StopReason::ToolUse,
            ),
            text_response("done"),
        ]);

        let (_rx, handle) = agent_loop(
            vec![user_message("echo something")],
            AgentContext {
                messages: Vec::new(),
                tools: vec![Arc::new(echo_tool(executed.clone()))],
            },
            config,
            models,
            None,
        );
        handle.await.unwrap().unwrap();

        assert_eq!(*executed.lock().unwrap(), [serde_json::json!(123)]);
    }

    /// Oracle "should prepare tool arguments for validation": the
    /// prepareArguments shim runs before schema validation.
    #[tokio::test]
    async fn prepares_tool_arguments_for_validation() {
        let (models, faux, model) = faux_models();
        let executed = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let executed_writer = executed.clone();
        let mut tool = echo_tool(Arc::new(Mutex::new(Vec::new())));
        tool.name = "edit".into();
        tool.label = "Edit".into();
        tool.description = "Edit tool".into();
        tool.parameters = serde_json::json!({
            "type": "object",
            "properties": {
                "edits": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "oldText": {"type": "string"},
                            "newText": {"type": "string"}
                        },
                        "required": ["oldText", "newText"]
                    }
                }
            },
            "required": ["edits"]
        });
        tool.execute = Arc::new(
            move |_id: String,
                  params: serde_json::Value,
                  _signal: Option<CancellationToken>,
                  _on_update: Option<Arc<AgentToolUpdateCallback>>| {
                let executed = executed_writer.clone();
                Box::pin(async move {
                    executed.lock().unwrap().push(params["edits"].clone());
                    let count = params["edits"].as_array().map(Vec::len).unwrap_or(0);
                    Ok(AgentToolResult {
                        content: vec![text_block(&format!("edited {count}"))],
                        details: Some(serde_json::json!({"count": count})),
                        ..AgentToolResult::default()
                    })
                })
            },
        );
        tool.prepare_arguments = Some(Arc::new(|args: serde_json::Value| {
            let mut edits: Vec<serde_json::Value> = args
                .get("edits")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let (Some(old), Some(new)) = (
                args.get("oldText").and_then(serde_json::Value::as_str),
                args.get("newText").and_then(serde_json::Value::as_str),
            ) {
                edits.push(serde_json::json!({"oldText": old, "newText": new}));
            }
            serde_json::json!({"edits": edits})
        }));
        faux.set_responses(vec![
            tool_call_response(
                vec![(
                    "tool-1",
                    "edit",
                    serde_json::json!({"oldText": "before", "newText": "after"}),
                )],
                StopReason::ToolUse,
            ),
            text_response("done"),
        ]);

        let (_rx, handle) = agent_loop(
            vec![user_message("edit something")],
            AgentContext {
                messages: Vec::new(),
                tools: vec![Arc::new(tool)],
            },
            identity_config(model),
            models,
            None,
        );
        handle.await.unwrap().unwrap();

        assert_eq!(
            *executed.lock().unwrap(),
            [serde_json::json!([{"oldText": "before", "newText": "after"}])]
        );
    }

    /// Oracle "should emit tool_execution_end in completion order but persist
    /// tool results in source order" (parallel default).
    #[tokio::test]
    async fn emits_tool_execution_end_in_completion_order_but_persists_in_source_order() {
        let (models, faux, model) = faux_models();
        let notify = Arc::new(Notify::new());
        let first_resolved = Arc::new(AtomicBool::new(false));
        let parallel_observed = Arc::new(AtomicBool::new(false));
        let executed = Arc::new(Mutex::new(Vec::<String>::new()));
        let tool = blocking_tool(
            "echo",
            "first",
            notify.clone(),
            first_resolved,
            parallel_observed.clone(),
            executed,
            None,
        );
        faux.set_responses(vec![
            tool_call_response(
                vec![
                    ("tool-1", "echo", serde_json::json!({"value": "first"})),
                    ("tool-2", "echo", serde_json::json!({"value": "second"})),
                ],
                StopReason::ToolUse,
            ),
            text_response("done"),
        ]);
        spawn_releaser(&notify, 50);

        let (rx, handle) = agent_loop(
            vec![user_message("echo both")],
            AgentContext {
                messages: Vec::new(),
                tools: vec![Arc::new(tool)],
            },
            identity_config(model),
            models,
            None,
        );
        let _messages = handle.await.unwrap().unwrap();
        let events = collect_events(rx).await;

        assert!(parallel_observed.load(Ordering::SeqCst));
        let end_ids: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::ToolExecutionEnd { tool_call_id, .. } => Some(tool_call_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(end_ids, ["tool-2", "tool-1"]);
        let result_ids: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::MessageEnd {
                    message: AgentMessage::ToolResult(tool_result),
                } => Some(tool_result.tool_call_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(result_ids, ["tool-1", "tool-2"]);
        let turn_ids: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::TurnEnd { tool_results, .. } => Some(
                    tool_results
                        .iter()
                        .map(|tool_result| tool_result.tool_call_id.as_str())
                        .collect::<Vec<_>>(),
                ),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(turn_ids, ["tool-1", "tool-2"]);
    }

    /// Oracle "should force sequential execution when a tool has
    /// executionMode=sequential even with default parallel config".
    #[tokio::test]
    async fn forces_sequential_when_a_tool_declares_sequential_mode() {
        let (models, faux, model) = faux_models();
        let notify = Arc::new(Notify::new());
        let first_resolved = Arc::new(AtomicBool::new(false));
        let parallel_observed = Arc::new(AtomicBool::new(false));
        let executed = Arc::new(Mutex::new(Vec::<String>::new()));
        let tool = blocking_tool(
            "slow",
            "first",
            notify.clone(),
            first_resolved,
            parallel_observed.clone(),
            executed,
            Some(ToolExecutionMode::Sequential),
        );
        faux.set_responses(vec![
            tool_call_response(
                vec![
                    ("tool-1", "slow", serde_json::json!({"value": "first"})),
                    ("tool-2", "slow", serde_json::json!({"value": "second"})),
                ],
                StopReason::ToolUse,
            ),
            text_response("done"),
        ]);
        spawn_releaser(&notify, 50);

        let (rx, handle) = agent_loop(
            vec![user_message("run both")],
            AgentContext {
                messages: Vec::new(),
                tools: vec![Arc::new(tool)],
            },
            // config is parallel by default, but the tool forces sequential
            identity_config(model),
            models,
            None,
        );
        let _messages = handle.await.unwrap().unwrap();
        let events = collect_events(rx).await;

        assert!(!parallel_observed.load(Ordering::SeqCst));
        let result_ids: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::MessageEnd {
                    message: AgentMessage::ToolResult(tool_result),
                } => Some(tool_result.tool_call_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(result_ids, ["tool-1", "tool-2"]);
    }

    /// Oracle "should force sequential execution when one of multiple tools
    /// has executionMode=sequential".
    #[tokio::test]
    async fn forces_sequential_when_one_of_multiple_tools_is_sequential() {
        let (models, faux, model) = faux_models();
        let notify = Arc::new(Notify::new());
        let execution_order = Arc::new(Mutex::new(Vec::<String>::new()));
        let slow = recording_tool(
            "slow",
            Some("a"),
            Some(notify.clone()),
            execution_order.clone(),
            Some(ToolExecutionMode::Sequential),
        );
        let fast = recording_tool("fast", None, None, execution_order.clone(), None);
        faux.set_responses(vec![
            tool_call_response(
                vec![
                    ("tool-1", "slow", serde_json::json!({"value": "a"})),
                    ("tool-2", "fast", serde_json::json!({"value": "b"})),
                ],
                StopReason::ToolUse,
            ),
            text_response("done"),
        ]);
        spawn_releaser(&notify, 50);

        let (_rx, handle) = agent_loop(
            vec![user_message("run both")],
            AgentContext {
                messages: Vec::new(),
                tools: vec![Arc::new(slow), Arc::new(fast)],
            },
            identity_config(model),
            models,
            None,
        );
        handle.await.unwrap().unwrap();

        assert_eq!(*execution_order.lock().unwrap(), ["slow:a", "fast:b"]);
    }

    /// Oracle "should allow parallel execution when all tools have
    /// executionMode=parallel".
    #[tokio::test]
    async fn allows_parallel_execution_when_all_tools_declare_parallel() {
        let (models, faux, model) = faux_models();
        let notify = Arc::new(Notify::new());
        let first_resolved = Arc::new(AtomicBool::new(false));
        let parallel_observed = Arc::new(AtomicBool::new(false));
        let executed = Arc::new(Mutex::new(Vec::<String>::new()));
        let tool = blocking_tool(
            "echo",
            "first",
            notify.clone(),
            first_resolved,
            parallel_observed.clone(),
            executed,
            Some(ToolExecutionMode::Parallel),
        );
        faux.set_responses(vec![
            tool_call_response(
                vec![
                    ("tool-1", "echo", serde_json::json!({"value": "first"})),
                    ("tool-2", "echo", serde_json::json!({"value": "second"})),
                ],
                StopReason::ToolUse,
            ),
            text_response("done"),
        ]);
        spawn_releaser(&notify, 50);

        let (_rx, handle) = agent_loop(
            vec![user_message("echo both")],
            AgentContext {
                messages: Vec::new(),
                tools: vec![Arc::new(tool)],
            },
            identity_config(model),
            models,
            None,
        );
        handle.await.unwrap().unwrap();

        assert!(parallel_observed.load(Ordering::SeqCst));
    }

    /// Oracle "should use prepareNextTurn snapshot before continuing".
    #[tokio::test]
    async fn uses_prepare_next_turn_snapshot_before_continuing() {
        let (models, faux, model) = faux_models();
        let prepare_calls = Arc::new(AtomicUsize::new(0));
        let prepared = Arc::new(AtomicBool::new(false));
        let saw_guidance = Arc::new(AtomicBool::new(false));
        let (prepare_writer, prepared_flag) = (prepare_calls.clone(), prepared.clone());
        let guidance_writer = saw_guidance.clone();
        let mut config = identity_config(model);
        config.prepare_next_turn = Some(Arc::new(move |context: PrepareNextTurnContext| {
            let (prepare_writer, prepared_flag) = (prepare_writer.clone(), prepared_flag.clone());
            Box::pin(async move {
                prepare_writer.fetch_add(1, Ordering::SeqCst);
                if prepared_flag.swap(true, Ordering::SeqCst) {
                    return None;
                }
                Some(AgentLoopTurnUpdate {
                    context: Some(context.context),
                    messages: Some(vec![AgentMessage::System(SystemMessage {
                        content: StringOrBlocks::Text("updated guidance".into()),
                        sections: None,
                        tools_added: None,
                        tools_removed: None,
                        timestamp: 1,
                    })]),
                    model: None,
                    thinking_level: None,
                })
            })
        }));
        let tool = echo_tool(Arc::new(Mutex::new(Vec::<serde_json::Value>::new())));
        faux.set_responses(vec![
            tool_call_response(
                vec![("tool-1", "echo", serde_json::json!({"value": "hello"}))],
                StopReason::ToolUse,
            ),
            FauxResponseStep::Factory(Arc::new(move |args: FauxFactoryArgs| {
                let guidance_writer = guidance_writer.clone();
                Box::pin(async move {
                    let has_guidance = args.context.messages().iter().any(|message| {
                        matches!(message, Message::System(system) if content_text(&system.content) == "updated guidance")
                    });
                    guidance_writer.store(has_guidance, Ordering::SeqCst);
                    Ok(faux_assistant_message("done", FauxMessageOptions::default()))
                })
            })),
        ]);

        let (_rx, handle) = agent_loop(
            vec![user_message("echo something")],
            AgentContext {
                messages: Vec::new(),
                tools: vec![Arc::new(tool)],
            },
            config,
            models,
            None,
        );
        handle.await.unwrap().unwrap();

        assert_eq!(call_count(&faux), 2);
        assert_eq!(prepare_calls.load(Ordering::SeqCst), 1);
        assert!(saw_guidance.load(Ordering::SeqCst));
    }

    /// Oracle "should stop after the current turn when shouldStopAfterTurn
    /// returns true", including the exact event sequence. Upstream pins
    /// steeringPolls == 1 (the loop-start poll) and followUpPolls == 0: the
    /// hook exits before the post-turn steering poll and the follow-up poll.
    #[tokio::test]
    async fn stops_after_the_current_turn_when_should_stop_after_turn_is_true() {
        let (models, faux, model) = faux_models();
        let executed = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let steering_polls = Arc::new(AtomicUsize::new(0));
        let follow_up_polls = Arc::new(AtomicUsize::new(0));
        let callback_tool_result_ids = Arc::new(Mutex::new(Vec::<String>::new()));
        let callback_context_roles = Arc::new(Mutex::new(Vec::<String>::new()));
        let (steering_writer, follow_up_writer) = (steering_polls.clone(), follow_up_polls.clone());
        let (ids_writer, roles_writer) = (
            callback_tool_result_ids.clone(),
            callback_context_roles.clone(),
        );
        let mut config = identity_config(model);
        config.get_steering_messages = Some(Arc::new(move || {
            let steering_writer = steering_writer.clone();
            Box::pin(async move {
                steering_writer.fetch_add(1, Ordering::SeqCst);
                Vec::new()
            })
        }));
        config.get_follow_up_messages = Some(Arc::new(move || {
            let follow_up_writer = follow_up_writer.clone();
            Box::pin(async move {
                follow_up_writer.fetch_add(1, Ordering::SeqCst);
                vec![user_message("follow up should stay queued")]
            })
        }));
        config.should_stop_after_turn =
            Some(Arc::new(move |context: ShouldStopAfterTurnContext| {
                let (ids_writer, roles_writer) = (ids_writer.clone(), roles_writer.clone());
                Box::pin(async move {
                    *ids_writer.lock().unwrap() = context
                        .tool_results
                        .iter()
                        .map(|tool_result| tool_result.tool_call_id.clone())
                        .collect();
                    *roles_writer.lock().unwrap() = context
                        .context
                        .messages
                        .iter()
                        .map(|message| message.role().to_string())
                        .collect();
                    true
                })
            }));
        faux.set_responses(vec![tool_call_response(
            vec![("tool-1", "echo", serde_json::json!({"value": "hello"}))],
            StopReason::ToolUse,
        )]);

        let (rx, handle) = agent_loop(
            vec![user_message("echo something")],
            AgentContext {
                messages: Vec::new(),
                tools: vec![Arc::new(echo_tool(executed.clone()))],
            },
            config,
            models,
            None,
        );
        let messages = handle.await.unwrap().unwrap();
        let events = collect_events(rx).await;

        assert_eq!(call_count(&faux), 1);
        assert_eq!(*executed.lock().unwrap(), [serde_json::json!("hello")]);
        // Upstream counts: the loop-start steering poll runs once
        // (agent-loop.ts:174); shouldStopAfterTurn exits before the post-turn
        // steering poll (agent-loop.ts:263) and the follow-up poll
        // (agent-loop.ts:267).
        assert_eq!(steering_polls.load(Ordering::SeqCst), 1);
        assert_eq!(follow_up_polls.load(Ordering::SeqCst), 0);
        assert_eq!(*callback_tool_result_ids.lock().unwrap(), ["tool-1"]);
        assert_eq!(
            *callback_context_roles.lock().unwrap(),
            ["system", "user", "assistant", "toolResult"]
        );
        assert_eq!(
            role_names(&messages),
            ["system", "user", "assistant", "toolResult"]
        );
        assert_eq!(
            canonical_event_names(&events),
            [
                "agent_start",
                "turn_start",
                "message_start",
                "message_end",
                "message_start",
                "message_end",
                "message_start",
                "message_update*",
                "message_end",
                "tool_execution_start",
                "tool_execution_end",
                "message_start",
                "message_end",
                "turn_end",
                "agent_end",
            ]
        );
    }

    /// Oracle "should inject queued messages after all tool calls complete":
    /// both tools of the assistant message execute before the queued steering
    /// message is injected, and the follow-up LLM call sees it in context.
    #[tokio::test]
    async fn injects_queued_messages_after_all_tool_calls_complete() {
        let (models, faux, model) = faux_models();
        let executed = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let executed_writer = executed.clone();
        let queued_delivered = Arc::new(AtomicBool::new(false));
        let delivered_writer = queued_delivered.clone();
        let mut config = identity_config(model);
        config.tool_execution = Some(ToolExecutionMode::Sequential);
        // Return the steering message after tool execution has started
        // (upstream hook shape, agent-loop.test.ts:747-754).
        config.get_steering_messages = Some(Arc::new(move || {
            let (executed_writer, delivered_writer) =
                (executed_writer.clone(), delivered_writer.clone());
            Box::pin(async move {
                if !executed_writer.lock().unwrap().is_empty()
                    && !delivered_writer.swap(true, Ordering::SeqCst)
                {
                    vec![user_message("interrupt")]
                } else {
                    Vec::new()
                }
            })
        }));
        let saw_interrupt = Arc::new(AtomicBool::new(false));
        let saw_writer = saw_interrupt.clone();
        faux.set_responses(vec![
            tool_call_response(
                vec![
                    ("tool-1", "echo", serde_json::json!({"value": "first"})),
                    ("tool-2", "echo", serde_json::json!({"value": "second"})),
                ],
                StopReason::ToolUse,
            ),
            FauxResponseStep::Factory(Arc::new(move |args: FauxFactoryArgs| {
                let saw_writer = saw_writer.clone();
                Box::pin(async move {
                    let saw = args.context.messages().iter().any(|message| {
                        matches!(message, Message::User(user) if content_text(&user.content) == "interrupt")
                    });
                    saw_writer.store(saw, Ordering::SeqCst);
                    Ok(faux_assistant_message("done", FauxMessageOptions::default()))
                })
            })),
        ]);

        let (rx, handle) = agent_loop(
            vec![user_message("start")],
            AgentContext {
                messages: Vec::new(),
                tools: vec![Arc::new(echo_tool(executed.clone()))],
            },
            config,
            models,
            None,
        );
        handle.await.unwrap().unwrap();
        let events = collect_events(rx).await;

        // Both tools should execute before steering is injected
        assert_eq!(
            *executed.lock().unwrap(),
            [serde_json::json!("first"), serde_json::json!("second")]
        );
        let tool_ends: Vec<bool> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::ToolExecutionEnd { is_error, .. } => Some(*is_error),
                _ => None,
            })
            .collect();
        assert_eq!(tool_ends, [false, false]);

        // Queued message appears in events after both tool result messages
        let sequence: Vec<String> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::MessageStart { message } => match message {
                    AgentMessage::ToolResult(tool_result) => {
                        Some(format!("tool:{}", tool_result.tool_call_id))
                    }
                    AgentMessage::User(user) => Some(content_text(&user.content)),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        let interrupt_index = sequence
            .iter()
            .position(|entry| entry == "interrupt")
            .expect("interrupt injected");
        let tool_1_index = sequence
            .iter()
            .position(|entry| entry == "tool:tool-1")
            .expect("tool-1 result");
        let tool_2_index = sequence
            .iter()
            .position(|entry| entry == "tool:tool-2")
            .expect("tool-2 result");
        assert!(tool_1_index < interrupt_index);
        assert!(tool_2_index < interrupt_index);

        // Interrupt message was in context when the second LLM call was made
        assert!(saw_interrupt.load(Ordering::SeqCst));
    }

    /// Upstream "continue() should process queued follow-up messages after an
    /// assistant turn" (agent.test.ts:795) at the loop level: the follow-up
    /// hook is consulted when the agent would stop, its message is injected,
    /// and another turn runs.
    #[tokio::test]
    async fn processes_follow_up_messages_when_the_agent_would_stop() {
        let (models, faux, model) = faux_models();
        let follow_up_polls = Arc::new(AtomicUsize::new(0));
        let polls_writer = follow_up_polls.clone();
        let mut config = identity_config(model);
        config.get_follow_up_messages = Some(Arc::new(move || {
            let polls_writer = polls_writer.clone();
            Box::pin(async move {
                let poll = polls_writer.fetch_add(1, Ordering::SeqCst);
                if poll == 0 {
                    vec![user_message("follow up")]
                } else {
                    Vec::new()
                }
            })
        }));
        faux.set_responses(vec![text_response("first"), text_response("second")]);

        let (rx, handle) = agent_loop(
            vec![user_message("start")],
            AgentContext::default(),
            config,
            models,
            None,
        );
        let messages = handle.await.unwrap().unwrap();
        let events = collect_events(rx).await;

        assert_eq!(call_count(&faux), 2);
        assert_eq!(follow_up_polls.load(Ordering::SeqCst), 2);
        // No tools in the context, so no tool-declaration system message.
        assert_eq!(
            role_names(&messages),
            ["user", "assistant", "user", "assistant"]
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, AgentEvent::TurnStart))
                .count(),
            2
        );
    }

    /// Upstream one-at-a-time steering semantics (agent.test.ts:833-872 at
    /// the loop level): each drain point injects exactly one queued message,
    /// so two queued steering messages take two turns.
    #[tokio::test]
    async fn steering_one_at_a_time_drains_one_message_per_poll() {
        let (models, faux, model) = faux_models();
        let queue = Arc::new(Mutex::new(PendingMessageQueue::new(QueueMode::OneAtATime)));
        {
            let mut queued = queue.lock().unwrap();
            queued.enqueue(user_message("steer 1"));
            queued.enqueue(user_message("steer 2"));
        }
        let queue_writer = queue.clone();
        let mut config = identity_config(model);
        config.get_steering_messages = Some(Arc::new(move || {
            let queue_writer = queue_writer.clone();
            Box::pin(async move { queue_writer.lock().unwrap().drain() })
        }));
        faux.set_responses(vec![text_response("first"), text_response("second")]);

        let (_rx, handle) = agent_loop(
            vec![user_message("start")],
            AgentContext::default(),
            config,
            models,
            None,
        );
        let messages = handle.await.unwrap().unwrap();

        assert_eq!(call_count(&faux), 2);
        let user_texts: Vec<String> = messages
            .iter()
            .filter_map(|message| match message {
                AgentMessage::User(user) => Some(content_text(&user.content)),
                _ => None,
            })
            .collect();
        assert_eq!(user_texts, ["start", "steer 1", "steer 2"]);
    }

    /// Upstream "all" steering mode: a drain point injects the whole queue,
    /// so both queued steering messages land in one turn with one LLM call.
    #[tokio::test]
    async fn steering_all_mode_drains_the_whole_queue_in_one_turn() {
        let (models, faux, model) = faux_models();
        let queue = Arc::new(Mutex::new(PendingMessageQueue::new(QueueMode::All)));
        {
            let mut queued = queue.lock().unwrap();
            queued.enqueue(user_message("steer 1"));
            queued.enqueue(user_message("steer 2"));
        }
        let queue_writer = queue.clone();
        let mut config = identity_config(model);
        config.get_steering_messages = Some(Arc::new(move || {
            let queue_writer = queue_writer.clone();
            Box::pin(async move { queue_writer.lock().unwrap().drain() })
        }));
        faux.set_responses(vec![text_response("only response")]);

        let (_rx, handle) = agent_loop(
            vec![user_message("start")],
            AgentContext::default(),
            config,
            models,
            None,
        );
        let messages = handle.await.unwrap().unwrap();

        assert_eq!(call_count(&faux), 1);
        // No tools in the context, so no tool-declaration system message.
        assert_eq!(role_names(&messages), ["user", "user", "user", "assistant"]);
        let user_texts: Vec<String> = messages
            .iter()
            .filter_map(|message| match message {
                AgentMessage::User(user) => Some(content_text(&user.content)),
                _ => None,
            })
            .collect();
        assert_eq!(user_texts, ["start", "steer 1", "steer 2"]);
    }

    /// Follow-up is consulted only when there are no more tool calls and no
    /// steering messages (README "Steering and Follow-up"): the queued
    /// steering message turns first; the follow-up turns after the steering
    /// queue empties.
    #[tokio::test]
    async fn steering_is_drained_before_the_follow_up_queue() {
        let (models, faux, model) = faux_models();
        let queue = Arc::new(Mutex::new(PendingMessageQueue::new(QueueMode::OneAtATime)));
        queue.lock().unwrap().enqueue(user_message("steer"));
        let queue_writer = queue.clone();
        let follow_up_polls = Arc::new(AtomicUsize::new(0));
        let polls_writer = follow_up_polls.clone();
        let mut config = identity_config(model);
        config.get_steering_messages = Some(Arc::new(move || {
            let queue_writer = queue_writer.clone();
            Box::pin(async move { queue_writer.lock().unwrap().drain() })
        }));
        config.get_follow_up_messages = Some(Arc::new(move || {
            let polls_writer = polls_writer.clone();
            Box::pin(async move {
                let poll = polls_writer.fetch_add(1, Ordering::SeqCst);
                if poll == 0 {
                    vec![user_message("follow up")]
                } else {
                    Vec::new()
                }
            })
        }));
        faux.set_responses(vec![text_response("first"), text_response("second")]);

        let (_rx, handle) = agent_loop(
            vec![user_message("start")],
            AgentContext::default(),
            config,
            models,
            None,
        );
        let messages = handle.await.unwrap().unwrap();

        assert_eq!(call_count(&faux), 2);
        let user_texts: Vec<String> = messages
            .iter()
            .filter_map(|message| match message {
                AgentMessage::User(user) => Some(content_text(&user.content)),
                _ => None,
            })
            .collect();
        assert_eq!(user_texts, ["start", "steer", "follow up"]);
    }

    /// Steering queued while `prepareNextTurn` runs is picked up before the
    /// next turn (agent-loop.ts:200-204): the post-turn poll returned
    /// nothing, so the preparation-time re-poll delivers the message.
    #[tokio::test]
    async fn steering_queued_during_preparation_is_picked_up() {
        let (models, faux, model) = faux_models();
        let queue = Arc::new(Mutex::new(PendingMessageQueue::new(QueueMode::OneAtATime)));
        let prepare_queue = queue.clone();
        let poll_queue = queue.clone();
        let enqueued = Arc::new(AtomicBool::new(false));
        let enqueued_writer = enqueued.clone();
        let mut config = identity_config(model);
        config.prepare_next_turn = Some(Arc::new(move |_context: PrepareNextTurnContext| {
            let (prepare_queue, enqueued_writer) = (prepare_queue.clone(), enqueued_writer.clone());
            Box::pin(async move {
                // The user steers while preparation (e.g. compaction) runs.
                if !enqueued_writer.swap(true, Ordering::SeqCst) {
                    prepare_queue
                        .lock()
                        .unwrap()
                        .enqueue(user_message("late steer"));
                }
                None
            })
        }));
        config.get_steering_messages = Some(Arc::new(move || {
            let poll_queue = poll_queue.clone();
            Box::pin(async move { poll_queue.lock().unwrap().drain() })
        }));
        faux.set_responses(vec![
            tool_call_response(
                vec![("tool-1", "echo", serde_json::json!({"value": "hello"}))],
                StopReason::ToolUse,
            ),
            text_response("done"),
        ]);

        let (_rx, handle) = agent_loop(
            vec![user_message("start")],
            AgentContext {
                messages: Vec::new(),
                tools: vec![Arc::new(echo_tool(Arc::new(Mutex::new(Vec::new()))))],
            },
            config,
            models,
            None,
        );
        let messages = handle.await.unwrap().unwrap();

        assert_eq!(call_count(&faux), 2);
        let user_texts: Vec<String> = messages
            .iter()
            .filter_map(|message| match message {
                AgentMessage::User(user) => Some(content_text(&user.content)),
                _ => None,
            })
            .collect();
        assert_eq!(user_texts, ["start", "late steer"]);
    }

    /// Oracle "should stop after a tool batch when every tool result sets
    /// terminate=true".
    #[tokio::test]
    async fn stops_after_a_tool_batch_when_every_result_terminates() {
        let (models, faux, model) = faux_models();
        let executed = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let executed_writer = executed.clone();
        let tool = AgentTool {
            name: "echo".into(),
            label: "Echo".into(),
            description: "Echo tool".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"value": {"type": "string"}},
                "required": ["value"]
            }),
            execute: Arc::new(
                move |_id: String,
                      params: serde_json::Value,
                      _signal: Option<CancellationToken>,
                      _on_update: Option<Arc<AgentToolUpdateCallback>>| {
                    let executed = executed_writer.clone();
                    Box::pin(async move {
                        let value = params["value"].as_str().unwrap_or_default().to_string();
                        executed.lock().unwrap().push(params["value"].clone());
                        Ok(AgentToolResult {
                            content: vec![text_block(&format!("echoed: {value}"))],
                            details: Some(serde_json::json!({"value": value})),
                            usage: None,
                            terminate: Some(true),
                        })
                    })
                },
            ),
            constrained_sampling: None,
            prepare_arguments: None,
            replay: None,
            execution_mode: None,
        };
        faux.set_responses(vec![tool_call_response(
            vec![("tool-1", "echo", serde_json::json!({"value": "hello"}))],
            StopReason::ToolUse,
        )]);

        let (rx, handle) = agent_loop(
            vec![user_message("echo something")],
            AgentContext {
                messages: Vec::new(),
                tools: vec![Arc::new(tool)],
            },
            identity_config(model),
            models,
            None,
        );
        let messages = handle.await.unwrap().unwrap();
        let events = collect_events(rx).await;

        assert_eq!(call_count(&faux), 1);
        assert_eq!(
            role_names(&messages),
            ["system", "user", "assistant", "toolResult"]
        );
        assert_eq!(
            event_names(&events)
                .iter()
                .filter(|name| **name == "turn_end")
                .count(),
            1
        );
        assert_eq!(*executed.lock().unwrap(), [serde_json::json!("hello")]);
    }

    /// Oracle "should stop after a blocked tool call when beforeToolCall sets
    /// terminate=true".
    #[tokio::test]
    async fn stops_after_a_blocked_tool_call_with_terminate_hint() {
        let (models, faux, model) = faux_models();
        let executed = Arc::new(AtomicBool::new(false));
        let executed_writer = executed.clone();
        let tool = AgentTool {
            name: "echo".into(),
            label: "Echo".into(),
            description: "Echo tool".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"value": {"type": "string"}},
                "required": ["value"]
            }),
            execute: Arc::new(
                move |_id: String,
                      _params: serde_json::Value,
                      _signal: Option<CancellationToken>,
                      _on_update: Option<Arc<AgentToolUpdateCallback>>| {
                    let executed = executed_writer.clone();
                    Box::pin(async move {
                        executed.store(true, Ordering::SeqCst);
                        Ok(AgentToolResult {
                            content: vec![text_block("should not execute")],
                            details: Some(serde_json::json!({"value": "unexpected"})),
                            ..AgentToolResult::default()
                        })
                    })
                },
            ),
            constrained_sampling: None,
            prepare_arguments: None,
            replay: None,
            execution_mode: None,
        };
        let mut config = identity_config(model);
        config.before_tool_call = Some(Arc::new(|_hook: BeforeToolCallContext| {
            Box::pin(async move {
                BeforeToolCallOutcome {
                    args: None,
                    result: Some(BeforeToolCallResult {
                        block: Some(true),
                        reason: Some("Blocked by policy".into()),
                        terminate: Some(true),
                    }),
                }
            })
        }));
        faux.set_responses(vec![tool_call_response(
            vec![("tool-1", "echo", serde_json::json!({"value": "hello"}))],
            StopReason::ToolUse,
        )]);

        let (rx, handle) = agent_loop(
            vec![user_message("echo something")],
            AgentContext {
                messages: Vec::new(),
                tools: vec![Arc::new(tool)],
            },
            config,
            models,
            None,
        );
        let messages = handle.await.unwrap().unwrap();
        collect_events(rx).await;

        assert!(!executed.load(Ordering::SeqCst));
        assert_eq!(call_count(&faux), 1);
        let tool_result = messages
            .iter()
            .find_map(|message| match message {
                AgentMessage::ToolResult(tool_result) => Some(tool_result.clone()),
                _ => None,
            })
            .expect("toolResult message");
        assert!(tool_result.is_error);
        let content = tool_result.content.first().cloned().expect("content");
        let TextOrImageBlock::Text(text) = content else {
            panic!("expected text block");
        };
        assert_eq!(text.text, "Blocked by policy");
    }

    /// Oracle "should continue after a mixed batch with one terminating
    /// blocked call".
    #[tokio::test]
    async fn continues_after_a_mixed_batch_with_one_terminating_blocked_call() {
        let (models, faux, model) = faux_models();
        let executed = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let mut config = identity_config(model);
        config.tool_execution = Some(ToolExecutionMode::Parallel);
        config.before_tool_call = Some(Arc::new(|hook: BeforeToolCallContext| {
            Box::pin(async move {
                if hook.args["value"] == "first" {
                    BeforeToolCallOutcome {
                        args: None,
                        result: Some(BeforeToolCallResult {
                            block: Some(true),
                            reason: Some("Blocked first".into()),
                            terminate: Some(true),
                        }),
                    }
                } else {
                    BeforeToolCallOutcome::default()
                }
            })
        }));
        faux.set_responses(vec![
            tool_call_response(
                vec![
                    ("tool-1", "echo", serde_json::json!({"value": "first"})),
                    ("tool-2", "echo", serde_json::json!({"value": "second"})),
                ],
                StopReason::ToolUse,
            ),
            text_response("done"),
        ]);

        let (_rx, handle) = agent_loop(
            vec![user_message("echo both")],
            AgentContext {
                messages: Vec::new(),
                tools: vec![Arc::new(echo_tool(executed.clone()))],
            },
            config,
            models,
            None,
        );
        handle.await.unwrap().unwrap();

        assert_eq!(*executed.lock().unwrap(), [serde_json::json!("second")]);
        assert_eq!(call_count(&faux), 2);
    }

    /// Oracle "should continue after parallel tool calls when not all tool
    /// results terminate".
    #[tokio::test]
    async fn continues_after_parallel_calls_when_not_all_results_terminate() {
        let (models, faux, model) = faux_models();
        let executed = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let executed_writer = executed.clone();
        let tool = AgentTool {
            name: "echo".into(),
            label: "Echo".into(),
            description: "Echo tool".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"value": {"type": "string"}},
                "required": ["value"]
            }),
            execute: Arc::new(
                move |_id: String,
                      params: serde_json::Value,
                      _signal: Option<CancellationToken>,
                      _on_update: Option<Arc<AgentToolUpdateCallback>>| {
                    let executed = executed_writer.clone();
                    Box::pin(async move {
                        let value = params["value"].as_str().unwrap_or_default().to_string();
                        executed.lock().unwrap().push(params["value"].clone());
                        Ok(AgentToolResult {
                            content: vec![text_block(&format!("echoed: {value}"))],
                            details: Some(serde_json::json!({"value": value})),
                            usage: None,
                            terminate: Some(value == "first"),
                        })
                    })
                },
            ),
            constrained_sampling: None,
            prepare_arguments: None,
            replay: None,
            execution_mode: None,
        };
        let mut config = identity_config(model);
        config.tool_execution = Some(ToolExecutionMode::Parallel);
        faux.set_responses(vec![
            tool_call_response(
                vec![
                    ("tool-1", "echo", serde_json::json!({"value": "first"})),
                    ("tool-2", "echo", serde_json::json!({"value": "second"})),
                ],
                StopReason::ToolUse,
            ),
            text_response("done"),
        ]);

        let (_rx, handle) = agent_loop(
            vec![user_message("echo both")],
            AgentContext {
                messages: Vec::new(),
                tools: vec![Arc::new(tool)],
            },
            config,
            models,
            None,
        );
        let messages = handle.await.unwrap().unwrap();

        assert_eq!(call_count(&faux), 2);
        assert_eq!(
            role_names(&messages),
            [
                "system",
                "user",
                "assistant",
                "toolResult",
                "toolResult",
                "assistant"
            ]
        );
        assert_eq!(
            *executed.lock().unwrap(),
            [serde_json::json!("first"), serde_json::json!("second")]
        );
    }

    /// Oracle "should allow afterToolCall to mark a tool batch as
    /// terminating".
    #[tokio::test]
    async fn allows_after_tool_call_to_mark_a_batch_as_terminating() {
        let (models, faux, model) = faux_models();
        let mut config = identity_config(model);
        config.after_tool_call = Some(Arc::new(|_hook: AfterToolCallContext| {
            Box::pin(async move {
                Some(AfterToolCallResult {
                    terminate: Some(true),
                    ..AfterToolCallResult::default()
                })
            })
        }));
        faux.set_responses(vec![tool_call_response(
            vec![("tool-1", "echo", serde_json::json!({"value": "hello"}))],
            StopReason::ToolUse,
        )]);

        let (_rx, handle) = agent_loop(
            vec![user_message("echo something")],
            AgentContext {
                messages: Vec::new(),
                tools: vec![Arc::new(echo_tool(Arc::new(Mutex::new(Vec::new()))))],
            },
            config,
            models,
            None,
        );
        handle.await.unwrap().unwrap();

        assert_eq!(call_count(&faux), 1);
    }

    /// M1-carried maxTurns guard (upstream agent-loop.ts has no equivalent):
    /// a run that would start turn N+1 bails with `exceeded max_turns (N)`.
    #[tokio::test]
    async fn bails_when_the_run_exceeds_max_turns() {
        let (models, faux, model) = faux_models();
        let mut config = identity_config(model);
        config.max_turns = 2;
        let tool_call = tool_call_response(
            vec![("tool-1", "echo", serde_json::json!({"value": "hello"}))],
            StopReason::ToolUse,
        );
        faux.set_responses(vec![tool_call.clone(), tool_call]);

        let (rx, handle) = agent_loop(
            vec![user_message("echo something")],
            AgentContext {
                messages: Vec::new(),
                tools: vec![Arc::new(echo_tool(Arc::new(Mutex::new(Vec::new()))))],
            },
            config,
            models,
            None,
        );
        let error = handle.await.unwrap().expect_err("max turns exceeded");
        assert!(error.to_string().contains("exceeded max_turns (2)"));
        let events = collect_events(rx).await;
        assert_eq!(event_names(&events).last().copied(), Some("agent_end"));
        assert_eq!(call_count(&faux), 2);
    }

    /// Abort mid-batch (agent-loop.ts:531-533): after tool-1's execution the
    /// sequential executor breaks, so tool-2 never starts; the follow-up
    /// provider call sees the cancelled token and settles aborted
    /// (agent-loop.ts:221-225).
    #[tokio::test]
    async fn sequential_execution_skips_remaining_calls_after_an_abort() {
        let (models, faux, model) = faux_models();
        let executed = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let executed_writer = executed.clone();
        let token = CancellationToken::new();
        let tool = AgentTool {
            name: "echo".into(),
            label: "Echo".into(),
            description: "Echo tool".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"value": {"type": "string"}},
                "required": ["value"]
            }),
            execute: Arc::new(
                move |_id: String,
                      params: serde_json::Value,
                      signal: Option<CancellationToken>,
                      _on_update: Option<Arc<AgentToolUpdateCallback>>| {
                    let executed_writer = executed_writer.clone();
                    Box::pin(async move {
                        executed_writer
                            .lock()
                            .unwrap()
                            .push(params["value"].clone());
                        if params["value"] == "first" {
                            // Cancel while the first tool runs; the token the
                            // loop handed the tool is the run's token.
                            if let Some(signal) = signal {
                                signal.cancel();
                            }
                        }
                        Ok(AgentToolResult {
                            content: vec![text_block(&format!(
                                "echoed: {}",
                                params["value"].as_str().unwrap_or_default()
                            ))],
                            details: Some(serde_json::json!({"value": params["value"]})),
                            ..AgentToolResult::default()
                        })
                    })
                },
            ),
            constrained_sampling: None,
            prepare_arguments: None,
            replay: None,
            execution_mode: None,
        };
        let mut config = identity_config(model);
        config.tool_execution = Some(ToolExecutionMode::Sequential);
        // The second response is never streamed: the pre-cancelled token
        // settles the stream aborted before any event.
        faux.set_responses(vec![
            tool_call_response(
                vec![
                    ("tool-1", "echo", serde_json::json!({"value": "first"})),
                    ("tool-2", "echo", serde_json::json!({"value": "second"})),
                ],
                StopReason::ToolUse,
            ),
            text_response("never streamed"),
        ]);

        let (rx, handle) = agent_loop(
            vec![user_message("echo both")],
            AgentContext {
                messages: Vec::new(),
                tools: vec![Arc::new(tool)],
            },
            config,
            models,
            Some(token),
        );
        let messages = handle.await.unwrap().unwrap();
        let events = collect_events(rx).await;

        assert_eq!(*executed.lock().unwrap(), [serde_json::json!("first")]);
        let started: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::ToolExecutionStart { tool_call_id, .. } => Some(tool_call_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(started, ["tool-1"]);
        // The second provider request is rejected in auth setup by the
        // already-cancelled token ("auth operation cancelled", counted as 1
        // provider call); the loop ends the run with the failed assistant
        // message (agent-loop.ts:221-225).
        match messages.last() {
            Some(AgentMessage::Assistant(assistant)) => assert!(matches!(
                assistant.stop_reason,
                StopReason::Aborted | StopReason::Error
            )),
            other => panic!(
                "expected final assistant message, got {:?}",
                other.map(|m| m.role())
            ),
        }
        assert_eq!(call_count(&faux), 1);
    }

    /// Abort during parallel preflight (agent-loop.ts:691-697, 569-571): the
    /// `beforeToolCall` hook cancels the token, the post-hook check fails the
    /// call with "Operation aborted" without running the tool, and the
    /// preflight break means later calls never start.
    #[tokio::test]
    async fn parallel_preflight_fails_and_stops_after_an_abort() {
        let (models, faux, model) = faux_models();
        let executed = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let token = CancellationToken::new();
        let token_writer = token.clone();
        let mut config = identity_config(model);
        config.before_tool_call = Some(Arc::new(move |_hook: BeforeToolCallContext| {
            let token_writer = token_writer.clone();
            Box::pin(async move {
                token_writer.cancel();
                BeforeToolCallOutcome::default()
            })
        }));
        faux.set_responses(vec![
            tool_call_response(
                vec![
                    ("tool-1", "echo", serde_json::json!({"value": "first"})),
                    ("tool-2", "echo", serde_json::json!({"value": "second"})),
                ],
                StopReason::ToolUse,
            ),
            text_response("never streamed"),
        ]);

        let (rx, handle) = agent_loop(
            vec![user_message("echo both")],
            AgentContext {
                messages: Vec::new(),
                tools: vec![Arc::new(echo_tool(executed.clone()))],
            },
            config,
            models,
            Some(token),
        );
        let messages = handle.await.unwrap().unwrap();
        let events = collect_events(rx).await;

        assert!(executed.lock().unwrap().is_empty());
        let started: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::ToolExecutionStart { tool_call_id, .. } => Some(tool_call_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(started, ["tool-1"]);
        let aborted_end = events.iter().any(|event| match event {
            AgentEvent::ToolExecutionEnd {
                is_error, result, ..
            } => {
                *is_error
                    && result["content"][0]["text"]
                        .as_str()
                        .is_some_and(|text| text == "Operation aborted")
            }
            _ => false,
        });
        assert!(aborted_end);
        // The second provider request is rejected in auth setup by the
        // already-cancelled token ("auth operation cancelled", counted as 1
        // provider call); the loop ends the run with the failed assistant
        // message (agent-loop.ts:221-225).
        match messages.last() {
            Some(AgentMessage::Assistant(assistant)) => assert!(matches!(
                assistant.stop_reason,
                StopReason::Aborted | StopReason::Error
            )),
            other => panic!(
                "expected final assistant message, got {:?}",
                other.map(|m| m.role())
            ),
        }
        assert_eq!(call_count(&faux), 1);
    }

    /// Abort between parallel preflights (agent-loop.ts:576-584): tool-1 was
    /// preflighted before the token was cancelled, so its pending execution
    /// fails with "Operation aborted" without running the tool; tool-2's
    /// post-hook check fails it during preflight. The tool-1
    /// `tool_execution_end` emitted at execution time proves the closure path
    /// ran.
    #[tokio::test]
    async fn parallel_pending_execution_fails_after_an_abort() {
        let (models, faux, model) = faux_models();
        let executed = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let token = CancellationToken::new();
        let token_writer = token.clone();
        let hook_calls = Arc::new(AtomicUsize::new(0));
        let calls_writer = hook_calls.clone();
        let mut config = identity_config(model);
        config.before_tool_call = Some(Arc::new(move |_hook: BeforeToolCallContext| {
            let (token_writer, calls_writer) = (token_writer.clone(), calls_writer.clone());
            Box::pin(async move {
                if calls_writer.fetch_add(1, Ordering::SeqCst) == 1 {
                    // Second invocation: cancel before returning, so the
                    // post-hook check fails tool-2 and the preflight breaks.
                    token_writer.cancel();
                }
                BeforeToolCallOutcome::default()
            })
        }));
        faux.set_responses(vec![
            tool_call_response(
                vec![
                    ("tool-1", "echo", serde_json::json!({"value": "first"})),
                    ("tool-2", "echo", serde_json::json!({"value": "second"})),
                ],
                StopReason::ToolUse,
            ),
            text_response("never streamed"),
        ]);

        let (rx, handle) = agent_loop(
            vec![user_message("echo both")],
            AgentContext {
                messages: Vec::new(),
                tools: vec![Arc::new(echo_tool(executed.clone()))],
            },
            config,
            models,
            Some(token),
        );
        let messages = handle.await.unwrap().unwrap();
        let events = collect_events(rx).await;

        assert!(executed.lock().unwrap().is_empty());
        let ends: Vec<(&str, bool, String)> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::ToolExecutionEnd {
                    tool_call_id,
                    is_error,
                    result,
                    ..
                } => Some((
                    tool_call_id.as_str(),
                    *is_error,
                    result["content"][0]["text"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                )),
                _ => None,
            })
            .collect();
        // tool-2's end came from the preflight, tool-1's from the aborted
        // pending execution: completion order, both "Operation aborted".
        assert_eq!(
            ends,
            [
                ("tool-2", true, "Operation aborted".to_string()),
                ("tool-1", true, "Operation aborted".to_string()),
            ]
        );
        // The second provider request is rejected in auth setup by the
        // already-cancelled token ("auth operation cancelled", counted as 1
        // provider call); the loop ends the run with the failed assistant
        // message (agent-loop.ts:221-225).
        match messages.last() {
            Some(AgentMessage::Assistant(assistant)) => assert!(matches!(
                assistant.stop_reason,
                StopReason::Aborted | StopReason::Error
            )),
            other => panic!(
                "expected final assistant message, got {:?}",
                other.map(|m| m.role())
            ),
        }
        assert_eq!(call_count(&faux), 1);
    }

    /// A provider stream that settles aborted mid-flight ends the run with
    /// the aborted assistant message: `turn_end` + `agent_end`, no further
    /// LLM call (agent-loop.ts:221-225; upstream agent.test.ts abort tests
    /// push `{ type: "error", reason: "aborted" }` from the stream).
    #[tokio::test]
    async fn run_settles_with_turn_end_and_agent_end_when_the_stream_aborts() {
        // A throttled faux stream: one token per chunk at 50 tokens/second,
        // so a 4000-character response streams for about 320ms.
        let faux = faux_provider(FauxProviderOptions {
            tokens_per_second: Some(50.0),
            token_size: Some(FauxTokenSize {
                min: Some(1),
                max: Some(1),
            }),
            ..FauxProviderOptions::default()
        });
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(faux.provider.clone());
        let model = faux.get_model(None).expect("faux default model");
        faux.set_responses(vec![FauxResponseStep::Message(Box::new(
            faux_assistant_message("x".repeat(4000), FauxMessageOptions::default()),
        ))]);
        let models = Arc::new(models);

        let token = CancellationToken::new();
        let canceller = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            canceller.cancel();
        });

        let (rx, handle) = agent_loop(
            vec![user_message("Hello")],
            AgentContext::default(),
            identity_config(model),
            models,
            Some(token),
        );
        let messages = handle.await.unwrap().unwrap();
        let events = collect_events(rx).await;

        assert_eq!(call_count(&faux), 1);
        assert_eq!(role_names(&messages), ["user", "assistant"]);
        let last = messages.last().expect("assistant message");
        let AgentMessage::Assistant(assistant) = last else {
            panic!("expected assistant message, got {}", last.role());
        };
        assert_eq!(assistant.stop_reason, StopReason::Aborted);
        assert_eq!(
            assistant.error_message.as_deref(),
            Some("Request was aborted")
        );
        // The run ends at the aborted turn: one turn_end, then agent_end.
        let tail: Vec<&str> = event_names(&events)
            .iter()
            .rev()
            .take(2)
            .rev()
            .copied()
            .collect();
        assert_eq!(tail, ["turn_end", "agent_end"]);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, AgentEvent::TurnEnd { .. }))
                .count(),
            1
        );
    }

    /// M1 intent (mini-loop `stream_error_is_reported`): a provider stream
    /// that settles with an error is reported — the error text is preserved on
    /// the failed assistant message, and the run ends at that turn
    /// (agent-loop.ts:221-225), never converting the failure into toolResult
    /// messages.
    #[tokio::test]
    async fn stream_error_ends_the_run_with_the_reported_error_message() {
        let (models, faux, model) = faux_models();
        faux.set_responses(vec![FauxResponseStep::Factory(Arc::new(
            |_args: FauxFactoryArgs| {
                Box::pin(async move { Err("boom".to_string()) })
                    as BoxFuture<'static, Result<AssistantMessage, String>>
            },
        ))]);

        let (rx, handle) = agent_loop(
            vec![user_message("hi")],
            AgentContext::default(),
            identity_config(model),
            models,
            None,
        );
        let messages = handle.await.unwrap().unwrap();
        let events = collect_events(rx).await;

        assert_eq!(role_names(&messages), ["user", "assistant"]);
        let AgentMessage::Assistant(assistant) = &messages[1] else {
            panic!("expected assistant message, got {}", messages[1].role());
        };
        assert_eq!(assistant.stop_reason, StopReason::Error);
        assert_eq!(assistant.error_message.as_deref(), Some("boom"));
        // The failure is reported to listeners: the assistant message_end
        // carries the error text, then the run settles turn_end + agent_end.
        let reported = events.iter().any(|event| match event {
            AgentEvent::MessageEnd {
                message: AgentMessage::Assistant(assistant),
            } => assistant.error_message.as_deref() == Some("boom"),
            _ => false,
        });
        assert!(reported);
        let tail: Vec<&str> = event_names(&events)
            .iter()
            .rev()
            .take(2)
            .rev()
            .copied()
            .collect();
        assert_eq!(tail, ["turn_end", "agent_end"]);
    }

    /// The preflight abort checks (agent-loop.ts:691-697, 710-716) directly:
    /// with a cancelled token a call fails with "Operation aborted" after the
    /// hook and again before returning a prepared call (no-hook path).
    #[tokio::test]
    async fn prepare_fails_with_operation_aborted_when_the_token_is_cancelled() {
        let (models, _faux, model) = faux_models();
        let context = AgentContext {
            messages: Vec::new(),
            tools: vec![Arc::new(echo_tool(Arc::new(Mutex::new(Vec::new()))))],
        };
        let assistant = faux_assistant_message("hi", FauxMessageOptions::default());
        let AssistantBlock::ToolCall(tool_call) = faux_tool_call(
            "echo",
            serde_json::json!({"value": "hello"}),
            FauxToolCallOptions {
                id: Some("tool-1".into()),
            },
        ) else {
            panic!("expected tool call block");
        };

        // No hook: the pre-return check (agent-loop.ts:710-716) fires.
        let token = CancellationToken::new();
        token.cancel();
        let prepared = prepare_tool_call(
            &context,
            &assistant,
            tool_call,
            &identity_config(model.clone()),
            Some(&token),
        )
        .await;
        match prepared {
            PreparedToolCall::Immediate { result, is_error } => {
                assert!(is_error);
                let TextOrImageBlock::Text(text) = &result.content[0] else {
                    panic!("expected text block");
                };
                assert_eq!(text.text, "Operation aborted");
            }
            PreparedToolCall::Prepared { .. } => {
                panic!("cancelled token must not prepare a call");
            }
        }

        // With a hook: the post-hook check (agent-loop.ts:691-697) fires
        // ahead of any block decision.
        let mut config = identity_config(model);
        config.before_tool_call = Some(Arc::new(|_hook: BeforeToolCallContext| {
            Box::pin(async move {
                BeforeToolCallOutcome {
                    args: None,
                    result: Some(BeforeToolCallResult {
                        block: Some(true),
                        reason: Some("should not be reached".into()),
                        terminate: None,
                    }),
                }
            })
        }));
        let token = CancellationToken::new();
        token.cancel();
        let AssistantBlock::ToolCall(tool_call) = faux_tool_call(
            "echo",
            serde_json::json!({"value": "hello"}),
            FauxToolCallOptions {
                id: Some("tool-1".into()),
            },
        ) else {
            panic!("expected tool call block");
        };
        let prepared =
            prepare_tool_call(&context, &assistant, tool_call, &config, Some(&token)).await;
        match prepared {
            PreparedToolCall::Immediate { result, is_error } => {
                assert!(is_error);
                let TextOrImageBlock::Text(text) = &result.content[0] else {
                    panic!("expected text block");
                };
                assert_eq!(text.text, "Operation aborted");
            }
            PreparedToolCall::Prepared { .. } => {
                panic!("cancelled token must not prepare a call");
            }
        }
        let _ = models;
    }

    // ---- PendingMessageQueue (agent.ts:140-177) ----

    #[test]
    fn queue_one_at_a_time_drains_one_message_per_drain() {
        let mut queue = PendingMessageQueue::new(QueueMode::OneAtATime);
        assert!(!queue.has_items());
        assert!(queue.drain().is_empty());

        queue.enqueue(user_message("a"));
        queue.enqueue(user_message("b"));
        assert!(queue.has_items());

        let drained = queue.drain();
        assert_eq!(drained.len(), 1);
        assert!(
            matches!(&drained[0], AgentMessage::User(user) if content_text(&user.content) == "a")
        );
        assert!(queue.has_items());

        let drained = queue.drain();
        assert_eq!(drained.len(), 1);
        assert!(
            matches!(&drained[0], AgentMessage::User(user) if content_text(&user.content) == "b")
        );
        assert!(!queue.has_items());

        queue.clear();
        assert!(!queue.has_items());
        assert!(queue.drain().is_empty());
    }

    #[test]
    fn queue_all_mode_drains_every_message() {
        let mut queue = PendingMessageQueue::new(QueueMode::All);
        queue.enqueue(user_message("a"));
        queue.enqueue(user_message("b"));

        let drained = queue.drain();
        assert_eq!(drained.len(), 2);
        assert!(
            matches!(&drained[0], AgentMessage::User(user) if content_text(&user.content) == "a")
        );
        assert!(
            matches!(&drained[1], AgentMessage::User(user) if content_text(&user.content) == "b")
        );
        assert!(!queue.has_items());
        assert!(queue.drain().is_empty());
    }

    // ---- agentLoopContinue (oracle block, agent-loop.test.ts:1516) ----

    /// Oracle "should throw when context has no messages" and the assistant
    /// guard (agent-loop.ts:76-82).
    #[tokio::test]
    async fn continue_validates_the_context_before_running() {
        let (models, _faux, model) = faux_models();
        let error = agent_loop_continue(
            AgentContext::default(),
            identity_config(model.clone()),
            models.clone(),
            None,
        )
        .expect_err("empty context must not continue");
        assert!(error
            .to_string()
            .contains("Cannot continue: no messages in context"));

        let error = agent_loop_continue(
            AgentContext {
                messages: vec![AgentMessage::Assistant(faux_assistant_message(
                    "hi",
                    FauxMessageOptions::default(),
                ))],
                tools: Vec::new(),
            },
            identity_config(model),
            models,
            None,
        )
        .expect_err("assistant last message must not continue");
        assert!(error
            .to_string()
            .contains("Cannot continue from message role: assistant"));
    }

    /// Oracle "should continue from existing context without emitting user
    /// message events".
    #[tokio::test]
    async fn continues_from_existing_context_without_user_message_events() {
        let (models, faux, model) = faux_models();
        faux.set_responses(vec![text_response("Response")]);

        let (rx, handle) = agent_loop_continue(
            AgentContext {
                messages: vec![user_message("Hello")],
                tools: Vec::new(),
            },
            identity_config(model),
            models,
            None,
        )
        .expect("continue is valid");

        let messages = handle.await.unwrap().unwrap();
        let events = collect_events(rx).await;

        assert_eq!(messages.len(), 1);
        assert_eq!(role_names(&messages), ["assistant"]);
        let message_end_roles: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::MessageEnd { message } => Some(message.role()),
                _ => None,
            })
            .collect();
        assert_eq!(message_end_roles, ["assistant"]);
    }

    /// Oracle "should allow custom message types as last message (caller
    /// responsibility)".
    #[tokio::test]
    async fn continues_with_a_custom_last_message() {
        let (models, faux, model) = faux_models();
        let custom = AgentMessage::Custom(CustomAgentMessage {
            role: "custom".into(),
            data: {
                let mut data = serde_json::Map::new();
                data.insert("text".into(), "Hook content".into());
                data.insert("timestamp".into(), serde_json::json!(now_ms()));
                data
            },
        });
        let config = AgentLoopConfig::new(
            model,
            Arc::new(|messages: Vec<AgentMessage>| {
                Box::pin(async move {
                    messages
                        .iter()
                        .filter_map(|message| match message {
                            AgentMessage::Custom(custom) => {
                                let text = custom
                                    .data
                                    .get("text")
                                    .and_then(serde_json::Value::as_str)
                                    .unwrap_or_default();
                                Some(Message::User(UserMessage {
                                    content: StringOrBlocks::Text(text.to_string()),
                                    timestamp: now_ms(),
                                }))
                            }
                            other => other.to_message(),
                        })
                        .filter(|message| {
                            matches!(
                                message,
                                Message::User(_) | Message::Assistant(_) | Message::ToolResult(_)
                            )
                        })
                        .collect::<Vec<Message>>()
                })
            }),
        );
        faux.set_responses(vec![text_response("Response to custom message")]);

        let (_rx, handle) = agent_loop_continue(
            AgentContext {
                messages: vec![custom],
                tools: Vec::new(),
            },
            config,
            models,
            None,
        )
        .expect("custom last message is allowed");

        let messages = handle.await.unwrap().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(role_names(&messages), ["assistant"]);
    }

    // ---- M1-carried integration: invalid arguments and unknown tools ----

    /// M1 intent (mini-loop `invalid_args_and_unknown_tool_become_error_results`):
    /// invalid arguments and unknown tools both settle as error tool results
    /// without executing the tool, and the run continues to the final text turn.
    #[tokio::test]
    async fn invalid_arguments_and_unknown_tools_become_error_results() {
        #[derive(serde::Deserialize, schemars::JsonSchema)]
        struct EchoArgs {
            text: String,
        }
        fn typed_echo_tool(executed: Arc<Mutex<Vec<serde_json::Value>>>) -> AgentTool {
            make_tool("echo", "echo text back", move |a: EchoArgs| {
                let executed = executed.clone();
                Box::pin(async move {
                    executed.lock().unwrap().push(serde_json::json!(a.text));
                    Ok(AgentToolResult {
                        content: vec![text_block(&format!("echo: {}", a.text))],
                        ..AgentToolResult::default()
                    })
                })
            })
        }

        let (models, faux, model) = faux_models();
        let executed = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        faux.set_responses(vec![
            tool_call_response(
                vec![("t1", "echo", serde_json::json!({"wrong": "arg"}))],
                StopReason::ToolUse,
            ),
            tool_call_response(
                vec![("t2", "nope", serde_json::json!({}))],
                StopReason::ToolUse,
            ),
            text_response("done"),
        ]);

        let (rx, handle) = agent_loop(
            vec![user_message("go")],
            AgentContext {
                messages: Vec::new(),
                tools: vec![Arc::new(typed_echo_tool(executed.clone()))],
            },
            identity_config(model),
            models,
            None,
        );
        let events = collect_events(rx).await;
        handle.await.unwrap().unwrap();

        let errors: Vec<bool> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::ToolExecutionEnd { is_error, .. } => Some(*is_error),
                _ => None,
            })
            .collect();
        assert_eq!(errors, [true, true]);
        // Neither the invalid-arguments call nor the unknown tool executed.
        assert!(executed.lock().unwrap().is_empty());
        // The run continues to the final text turn.
        let final_assistant = events
            .iter()
            .rev()
            .find_map(|event| match event {
                AgentEvent::MessageEnd {
                    message: AgentMessage::Assistant(assistant),
                } => Some(assistant.clone()),
                _ => None,
            })
            .expect("a final assistant message_end");
        assert_eq!(
            final_assistant.content,
            vec![AssistantBlock::Text(
                crate::ai::types::content::TextContent {
                    text: "done".into(),
                    text_signature: None,
                }
            )],
        );
        assert_eq!(final_assistant.stop_reason, StopReason::Stop);
    }

    // ---- M1-carried integration: the loop against the real ApiImpls (wiremock) ----

    /// Static-key auth for the wire-test providers: the M1 wire tests passed
    /// the key directly through `ProviderConfig`; the port routes through the
    /// `Models` collection, whose auth resolution must produce the same key.
    struct StaticKeyAuth {
        key: String,
    }

    impl ApiKeyAuth for StaticKeyAuth {
        fn name(&self) -> &str {
            "static test key"
        }

        fn resolve<'a>(
            &'a self,
            _input: ApiKeyAuthInput<'a>,
        ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
            Box::pin(async move {
                Ok(Some(AuthResult {
                    auth: ModelAuth {
                        api_key: Some(self.key.clone()),
                        ..ModelAuth::default()
                    },
                    ..AuthResult::default()
                }))
            })
        }
    }

    /// A `Models` collection serving one config-declared model through one
    /// real API implementation (the M1 wire tests' direct `ApiImpl` + config).
    fn wire_models(api: Arc<dyn ApiImpl>, provider_id: &str, model: Model) -> Arc<Models> {
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(create_provider(CreateProviderOptions {
            filter_all_models: None,
            images: crate::ai::models::provider::ImagesImpls::new(),
            classifiers: crate::ai::models::provider::ClassifiersImpls::new(),
            id: provider_id.to_string(),
            name: None,
            base_url: None,
            headers: None,
            auth: ProviderAuth {
                api_key: Some(Arc::new(StaticKeyAuth { key: "k".into() })),
                oauth: None,
            },
            models: vec![crate::ai::types::AnyModel::Chat(model)],
            fetch_models: None,
            filter_models: None,
            api: ApiImpls::Single(api),
        }));
        Arc::new(models)
    }

    fn wire_model(api: &str, provider: &str, id: &str, base_url: &str) -> Model {
        Model {
            id: id.into(),
            name: id.into(),
            api: api.into(),
            provider: provider.into(),
            base_url: base_url.into(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![ModelInput::Text],
            cost: ModelCost::default(),
            context_window: 200_000,
            max_tokens: 8192,
            sampling_params: None,
            sampling_params_by_thinking_level: None,
            headers: None,
            compat: None,
            r#type: None,
            prompt_cache: None,
            input_limits: None,
        }
    }

    fn system_message(text: &str) -> AgentMessage {
        AgentMessage::System(SystemMessage {
            content: StringOrBlocks::Text(text.into()),
            sections: None,
            tools_added: None,
            tools_removed: None,
            timestamp: now_ms(),
        })
    }

    fn data_line(json: serde_json::Value) -> String {
        format!("data: {json}\n\n")
    }

    fn content_chunk(text: &str) -> serde_json::Value {
        serde_json::json!({"choices": [{"delta": {"content": text}}]})
    }

    fn finish_chunk(finish_reason: &str) -> serde_json::Value {
        serde_json::json!({"choices": [{"delta": {}, "finish_reason": finish_reason}]})
    }

    fn sse(body: &str) -> wiremock::ResponseTemplate {
        wiremock::ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(body.to_string())
    }

    fn streamed_deltas(events: &[AgentEvent]) -> String {
        events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::MessageUpdate {
                    assistant_message_event: AssistantMessageEvent::TextDelta { delta, .. },
                    ..
                } => Some(delta.clone()),
                _ => None,
            })
            .collect::<Vec<String>>()
            .join("")
    }

    /// M1 intent (`agent_runs_text_turn_on_openai_completions_api`): one full
    /// agent text turn over `openai-completions` — the streamed deltas reach
    /// the events, the stored assistant message carries the Model-stamped
    /// metadata, and the wire body replays the transcript (system message
    /// leading, model id, user prompt).
    #[tokio::test]
    async fn agent_runs_text_turn_on_openai_completions_api() {
        let server = wiremock::MockServer::start().await;
        let body = format!(
            "{}{}{}",
            data_line(content_chunk("he")),
            data_line(content_chunk("y")),
            data_line(finish_chunk("stop"))
        );
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/chat/completions"))
            .respond_with(sse(&format!("{body}\ndata: [DONE]\n\n")))
            .mount(&server)
            .await;

        let base = format!("{}/v1", server.uri());
        let model = wire_model("openai-completions", "openai", "gpt-test", &base);
        let (rx, handle) = agent_loop(
            vec![user_message("hello")],
            AgentContext {
                messages: vec![system_message("sys")],
                tools: Vec::new(),
            },
            identity_config(model.clone()),
            wire_models(Arc::new(OpenAiCompletions), "openai", model),
            None,
        );

        let events = collect_events(rx).await;
        let messages = handle.await.unwrap().unwrap();

        assert_eq!(streamed_deltas(&events), "hey");
        // The run's new messages: the context system message is replayed from
        // the transcript, not part of the run's new messages.
        assert_eq!(role_names(&messages), ["user", "assistant"]);
        let AgentMessage::Assistant(assistant) = &messages[1] else {
            panic!("expected assistant message, got {:?}", messages[1].role());
        };
        assert_eq!(assistant.api, "openai-completions");
        assert_eq!(assistant.provider, "openai");
        assert_eq!(assistant.model, "gpt-test");
        assert_eq!(assistant.stop_reason, StopReason::Stop);

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let sent: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(sent["model"], "gpt-test");
        assert_eq!(sent["messages"][0]["role"], "system");
        assert_eq!(sent["messages"][0]["content"], "sys");
        assert_eq!(sent["messages"][1]["role"], "user");
        assert_eq!(sent["messages"][1]["content"], "hello");
    }

    /// M1 intent (`agent_runs_tool_loop_on_openai_completions_api`): a
    /// two-turn tool loop over `openai-completions` — streamed tool-call
    /// fragments with split JSON arguments execute the tool, the result feeds
    /// back as a replayed tool message on turn 2, and the declaration system
    /// message leads both requests.
    #[tokio::test]
    async fn agent_runs_tool_loop_on_openai_completions_api() {
        #[derive(serde::Deserialize, schemars::JsonSchema)]
        struct EchoArgs {
            text: String,
        }
        fn typed_echo_tool() -> AgentTool {
            make_tool("echo", "echo text back", |a: EchoArgs| {
                Box::pin(async move {
                    Ok(AgentToolResult {
                        content: vec![text_block(&format!("echo: {}", a.text))],
                        ..AgentToolResult::default()
                    })
                })
            })
        }

        let server = wiremock::MockServer::start().await;
        let turn1 = format!(
            "{}{}{}",
            data_line(serde_json::json!({"choices": [{"delta": {"tool_calls": [
                {"index": 0, "id": "t1", "type": "function", "function": {"name": "echo", "arguments": "{\"text\":"}}
            ]}}]})),
            data_line(serde_json::json!({"choices": [{"delta": {"tool_calls": [
                {"index": 0, "function": {"arguments": "\"hi\"}"}}
            ]}}]})),
            data_line(finish_chunk("tool_calls")),
        );
        let turn2 = format!(
            "{}{}",
            data_line(content_chunk("echo said hi")),
            data_line(finish_chunk("stop"))
        );
        let served_turn1 = std::sync::atomic::AtomicBool::new(true);
        struct Alternate {
            turn1: String,
            turn2: String,
            served_turn1: std::sync::atomic::AtomicBool,
        }
        impl wiremock::Respond for Alternate {
            fn respond(&self, _req: &wiremock::Request) -> wiremock::ResponseTemplate {
                let first = self
                    .served_turn1
                    .swap(false, std::sync::atomic::Ordering::SeqCst);
                let body = if first { &self.turn1 } else { &self.turn2 };
                sse(&format!("{body}\ndata: [DONE]\n\n"))
            }
        }
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(Alternate {
                turn1,
                turn2,
                served_turn1,
            })
            .mount(&server)
            .await;

        let base = format!("{}/v1", server.uri());
        let model = wire_model("openai-completions", "openai", "gpt-test", &base);
        let (rx, handle) = agent_loop(
            vec![user_message("run echo")],
            AgentContext {
                messages: Vec::new(),
                tools: vec![Arc::new(typed_echo_tool())],
            },
            identity_config(model.clone()),
            wire_models(Arc::new(OpenAiCompletions), "openai", model),
            None,
        );

        let _events = collect_events(rx).await;
        let messages = handle.await.unwrap().unwrap();

        // system declaration, user, assistant(toolcall), toolResult, final
        assert_eq!(
            role_names(&messages),
            ["system", "user", "assistant", "toolResult", "assistant"]
        );
        let AgentMessage::Assistant(assistant) = &messages[2] else {
            panic!("expected assistant message, got {:?}", messages[2].role());
        };
        assert_eq!(assistant.stop_reason, StopReason::ToolUse);
        let AgentMessage::ToolResult(result) = &messages[3] else {
            panic!("expected tool result, got {:?}", messages[3].role());
        };
        assert_eq!(result.tool_call_id, "t1");
        assert_eq!(result.content, vec![text_block("echo: hi")],);
        // Turn 2 replays the assistant tool call and the tool result. The
        // declaration system message (empty content) collapses into the
        // request-level `tools` param on the wire, not a chat message.
        let requests = server.received_requests().await.unwrap();
        let second: serde_json::Value = serde_json::from_slice(&requests[1].body).unwrap();
        let roles: Vec<&str> = second["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert_eq!(roles, ["user", "assistant", "tool"]);
        assert_eq!(
            second["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["function"]["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["echo"]
        );
    }

    /// M1 intent (`agent_runs_text_turn_on_anthropic_messages_api`): the same
    /// text turn over `anthropic-messages` — stamps and SSE parsing hold at
    /// the agent level for the second wire protocol too.
    #[tokio::test]
    async fn agent_runs_text_turn_on_anthropic_messages_api() {
        let server = wiremock::MockServer::start().await;
        // Event data carries the upstream `type` field; the impl dispatches
        // on it (upstream parses `{type: eventName, ...}`).
        let ev = |name: &str, data: serde_json::Value| {
            let mut data = data;
            data["type"] = serde_json::json!(name);
            format!("event: {name}\ndata: {data}\n\n")
        };
        let body = format!(
            "{}{}{}{}{}{}",
            ev(
                "message_start",
                serde_json::json!({"message": {"id": "msg_test", "usage": {"input_tokens": 1}}})
            ),
            ev(
                "content_block_start",
                serde_json::json!({"index": 0, "content_block": {"type": "text", "text": ""}})
            ),
            ev(
                "content_block_delta",
                serde_json::json!({"index": 0, "delta": {"type": "text_delta", "text": "hey"}})
            ),
            ev("content_block_stop", serde_json::json!({"index": 0})),
            ev(
                "message_delta",
                serde_json::json!({"delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 1}})
            ),
            ev("message_stop", serde_json::json!({})),
        );
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/messages"))
            .respond_with(sse(&body))
            .mount(&server)
            .await;

        let base = server.uri();
        let model = wire_model("anthropic-messages", "anthropic", "claude-test", &base);
        let (rx, handle) = agent_loop(
            vec![user_message("hello")],
            AgentContext::default(),
            identity_config(model.clone()),
            wire_models(Arc::new(AnthropicMessages), "anthropic", model),
            None,
        );

        let events = collect_events(rx).await;
        let messages = handle.await.unwrap().unwrap();

        assert_eq!(streamed_deltas(&events), "hey");
        let AgentMessage::Assistant(assistant) = &messages[1] else {
            panic!("expected assistant message, got {:?}", messages[1].role());
        };
        assert_eq!(assistant.api, "anthropic-messages");
        assert_eq!(assistant.provider, "anthropic");
        assert_eq!(assistant.model, "claude-test");
        assert_eq!(assistant.stop_reason, StopReason::Stop);
    }

    /// M1 intent (`agent_runs_text_turn_on_openai_responses_api`): a third
    /// wire protocol at the agent level — `openai-responses` streams a text
    /// turn and stamps the Model metadata into the stored message.
    #[tokio::test]
    async fn agent_runs_text_turn_on_openai_responses_api() {
        let server = wiremock::MockServer::start().await;
        let data = |value: serde_json::Value| format!("data: {value}\n\n");
        let body = format!(
            "{}{}{}{}{}",
            data(serde_json::json!({
                "type": "response.created",
                "response": {"id": "resp_ok"}
            })),
            data(serde_json::json!({
                "type": "response.output_item.added",
                "output_index": 0,
                "item": {"type": "message", "id": "msg_1", "role": "assistant", "content": []}
            })),
            data(serde_json::json!({
                "type": "response.output_text.delta",
                "output_index": 0,
                "item_id": "msg_1",
                "delta": "hey"
            })),
            data(serde_json::json!({
                "type": "response.output_item.done",
                "output_index": 0,
                "item": {"type": "message", "id": "msg_1", "role": "assistant",
                          "content": [{"type": "output_text", "text": "hey", "annotations": []}]}
            })),
            data(serde_json::json!({
                "type": "response.completed",
                "response": {"id": "resp_ok", "status": "completed",
                              "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}}
            })),
        );
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/responses"))
            .respond_with(sse(&body))
            .mount(&server)
            .await;

        let base = format!("{}/v1", server.uri());
        let model = wire_model("openai-responses", "openai", "gpt-test", &base);
        let (rx, handle) = agent_loop(
            vec![user_message("hello")],
            AgentContext::default(),
            identity_config(model.clone()),
            wire_models(Arc::new(OpenAiResponses), "openai", model),
            None,
        );

        let events = collect_events(rx).await;
        let messages = handle.await.unwrap().unwrap();

        assert_eq!(streamed_deltas(&events), "hey");
        let AgentMessage::Assistant(assistant) = &messages[1] else {
            panic!("expected assistant message, got {:?}", messages[1].role());
        };
        assert_eq!(assistant.api, "openai-responses");
        assert_eq!(assistant.provider, "openai");
        assert_eq!(assistant.model, "gpt-test");
        assert_eq!(assistant.stop_reason, StopReason::Stop);
        let requests = server.received_requests().await.unwrap();
        let sent: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(sent["model"], "gpt-test");
    }
}
