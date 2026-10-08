//! The Agent class from upstream `packages/agent/src/agent.ts` (M3a Task 4):
//! the stateful wrapper around the low-level agent loop. The Agent owns the
//! current transcript, emits lifecycle events to awaited subscribers,
//! executes tools through the loop, and exposes the queueing APIs for
//! steering and follow-up messages (agent.ts:182-607).
//!
//! Semantics carried from upstream (agent.ts + README "Agent" sections):
//! - `subscribe` listeners are awaited in registration order and are part of
//!   the run's settlement: `agent_end` is the last loop event, but the run is
//!   idle only after every awaited `agent_end` listener settles
//!   (agent.ts:552-560, 602-606). `wait_for_idle` and `prompt` both resolve
//!   after that.
//! - The emit sink the loop awaits is the Agent's own event processor: each
//!   event first reduces the runtime state (agent.ts:559-597), then listeners
//!   are awaited. Because the loop awaits the sink, README's raw-loop caveat
//!   ("observational streams do not wait for async handling") is exactly what
//!   the Agent class removes: assistant `message_end` processing is a
//!   barrier before tool preflight, so hooks observe state that already
//!   includes the assistant message.
//! - `reset` clears the transcript and queues while retaining the replayed
//!   prompt/tool baseline (agent.ts:347-361) and refuses while a run is
//!   active.
//! - The steering/follow-up queues are [`PendingMessageQueue`]s (ported with
//!   the loop, re-exported here) behind the `steer`/`follow_up`/`clear*`
//!   surface, drained through the loop's `getSteeringMessages`/
//!   `getFollowUpMessages` hooks with the configurable [`QueueMode`]s.
//!
//! Port deviations from the TypeScript source:
//! - `stream_fn` is an optional typed native async factory; omission retains
//!   the supplied `Arc<Models>` fallback instead of a process-global default.
//!   Dynamic `get_api_key`, request callbacks and transport are forwarded.
//!   Futures are poll-driven rather than eager JS promises; cancellation is
//!   cooperative through the active run token, including pending factories.
//! - `continue()` is [`Agent::continue_run`] (`continue` is a Rust keyword).
//! - `prompt(input, images?)` is [`Agent::prompt`] over [`PromptInput`]
//!   (with [`PromptInput::with_images`] for the image overload); the
//!   normalization produces the same user message shape (agent.ts:406-423).
//! - `steeringMode`/`followUpMode`/`sessionId`/... accessors are
//!   `set_steering_mode`/`runtime()`-style methods: the port keeps per-run
//!   options in a guarded [`AgentRuntimeOptions`] struct instead of public
//!   mutable fields.
//! - `reset` and the in-run guards return `anyhow::Result` instead of
//!   throwing, with the exact upstream error strings.
//! - Upstream `AbortSignal` is the port's [`CancellationToken`], handed to
//!   subscribers with each event (agent.ts:599-604).
//! - Upstream forwards the run's signal to the
//!   `beforeToolCall`/`afterToolCall`/`shouldStopAfterTurn`/
//!   `prepareNextTurn` hooks as a second argument (agent.ts:475-486); the
//!   port's hook signatures (types.ts ports) carry no signal parameter, so
//!   there is nothing to forward. `prepareNextTurn` and
//!   `prepareNextTurnWithContext` are one hook here — the port's hook
//!   already receives the turn context.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::bail;
use futures::future::BoxFuture;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::ai::models::Models;
use crate::ai::now_ms;
use crate::ai::transcript::{create_initial_system_message, get_current_system_message};
use crate::ai::types::content::{ImageContent, TextContent};
use crate::ai::types::message::{
    AssistantBlock, AssistantMessage, Message, StringOrBlocks, TextOrImageBlock, UserMessage,
};
use crate::ai::types::primitives::{StopReason, Transport, Usage};
use crate::ai::types::request_callbacks::RequestCallbacks;
use crate::ai::types::tool::Tool;

use super::agent_loop::{
    run_agent_loop, run_agent_loop_continue, AfterToolCallHook, AgentContext, AgentEventSink,
    AgentLoopConfig, BeforeToolCallHook, ConvertToLlmFn, MaxTurnsExceeded, PrepareNextTurnHook,
    ShouldStopAfterTurnHook, TransformContextFn,
};
use super::types::{
    unknown_model, AgentEvent, AgentInitialState, AgentMessage, AgentOptions, AgentState,
    GetApiKeyFn, StreamFn, ThinkingLevel, ToolExecutionMode,
};

/// Re-exported from the loop module per upstream ownership: the queue type
/// behind the Agent's `steer`/`follow_up` surface (upstream `agent.ts`
/// defines it; the port keeps it with the loop that drains it).
pub use super::agent_loop::PendingMessageQueue;
pub use super::types::QueueMode;

/// Upstream subscriber signature (agent.ts:265): `(event, signal) =>
/// Promise<void> | void`. Listeners are awaited in registration order.
pub type AgentListenerFn =
    dyn Fn(AgentEvent, CancellationToken) -> BoxFuture<'static, ()> + Send + Sync;

/// The subscriber list shared between the Agent and its unsubscribe handles.
type ListenerList = Arc<Mutex<Vec<(u64, Arc<AgentListenerFn>)>>>;

/// Upstream's `() => void` unsubscribe handle (agent.ts:267).
pub struct Unsubscribe {
    listeners: ListenerList,
    id: u64,
}

impl Unsubscribe {
    /// Remove the listener (upstream invoking the returned function).
    pub fn unsubscribe(self) {
        if let Ok(mut listeners) = self.listeners.lock() {
            listeners.retain(|(existing, _)| *existing != self.id);
        }
    }
}

/// Upstream `defaultConvertToLlm` (agent.ts:37-45): keep the standard roles,
/// dropping custom app messages (apps translate or convert them).
pub fn default_convert_to_llm(messages: Vec<AgentMessage>) -> BoxFuture<'static, Vec<Message>> {
    Box::pin(async move {
        messages
            .iter()
            .filter(|message| {
                matches!(
                    message.role(),
                    "system" | "user" | "assistant" | "toolResult"
                )
            })
            .filter_map(AgentMessage::to_message)
            .collect()
    })
}

/// The Agent's per-run option surface (upstream's public mutable fields on
/// `Agent`, agent.ts:194-229): read at the start of each run, mutable between
/// runs through [`Agent::runtime`].
#[derive(Clone)]
pub struct AgentRuntimeOptions {
    /// Native stream factory; `None` uses this Agent's Models collection.
    pub stream_fn: Option<Arc<StreamFn>>,
    /// Dynamic key resolution before each request.
    pub get_api_key: Option<Arc<GetApiKeyFn>>,
    /// Process-local payload/response hooks, preserving callback identity.
    pub callbacks: RequestCallbacks,
    /// Preferred transport forwarded to every stream call (default `auto`).
    pub transport: Transport,
    /// Transcript-to-LLM conversion before each provider call (upstream
    /// public `convertToLlm` field).
    pub convert_to_llm: Arc<ConvertToLlmFn>,
    /// Optional transcript transform before `convert_to_llm` (upstream
    /// public `transformContext` field).
    pub transform_context: Option<Arc<TransformContextFn>>,
    /// Called before a tool executes, after argument validation.
    pub before_tool_call: Option<Arc<BeforeToolCallHook>>,
    /// Called after a tool finishes, before result events.
    pub after_tool_call: Option<Arc<AfterToolCallHook>>,
    /// Called after `turn_end`; `true` stops the run.
    pub should_stop_after_turn: Option<Arc<ShouldStopAfterTurnHook>>,
    /// Called before the next turn when the loop continues.
    pub prepare_next_turn: Option<Arc<PrepareNextTurnHook>>,
    /// Session identifier forwarded to providers for cache-aware backends.
    pub session_id: Option<String>,
    /// Optional per-level thinking token budgets forwarded to the stream.
    pub thinking_budgets: Option<crate::ai::types::primitives::ThinkingBudgets>,
    /// Optional cap for provider-requested retry delays.
    pub max_retry_delay_ms: Option<u64>,
    /// Tool execution strategy for assistant messages with multiple tool
    /// calls.
    pub tool_execution: ToolExecutionMode,
}

impl std::fmt::Debug for AgentRuntimeOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentRuntimeOptions")
            .field("stream_fn", &self.stream_fn.is_some())
            .field("get_api_key", &self.get_api_key.is_some())
            .field("callbacks", &self.callbacks)
            .field("transport", &self.transport)
            .field("convert_to_llm", &"Arc<ConvertToLlmFn>")
            .field("transform_context", &self.transform_context.is_some())
            .field("before_tool_call", &self.before_tool_call.is_some())
            .field("after_tool_call", &self.after_tool_call.is_some())
            .field(
                "should_stop_after_turn",
                &self.should_stop_after_turn.is_some(),
            )
            .field("prepare_next_turn", &self.prepare_next_turn.is_some())
            .field("session_id", &self.session_id)
            .field("thinking_budgets", &self.thinking_budgets)
            .field("max_retry_delay_ms", &self.max_retry_delay_ms)
            .field("tool_execution", &self.tool_execution)
            .finish()
    }
}

/// One active run (upstream `ActiveRun`, agent.ts:176-180): the run's abort
/// token plus the idle signal `wait_for_idle` awaits.
struct ActiveRun {
    token: CancellationToken,
    idle: watch::Sender<bool>,
}

/// Upstream `prompt(input, images?)` overloads (agent.ts:364-366).
#[derive(Debug, Clone)]
pub enum PromptInput {
    /// Upstream `prompt(text, images?)`.
    Text {
        /// The prompt text.
        text: String,
        /// Optional image content appended after the text block.
        images: Vec<ImageContent>,
    },
    /// Upstream `prompt(AgentMessage)`.
    Message(Box<AgentMessage>),
    /// Upstream `prompt(AgentMessage[])`.
    Messages(Vec<AgentMessage>),
}

impl PromptInput {
    /// Upstream `prompt(text, images)`.
    pub fn with_images(text: impl Into<String>, images: Vec<ImageContent>) -> Self {
        Self::Text {
            text: text.into(),
            images,
        }
    }
}

impl From<&str> for PromptInput {
    fn from(text: &str) -> Self {
        Self::Text {
            text: text.to_string(),
            images: Vec::new(),
        }
    }
}

impl From<String> for PromptInput {
    fn from(text: String) -> Self {
        Self::Text {
            text,
            images: Vec::new(),
        }
    }
}

impl From<AgentMessage> for PromptInput {
    fn from(message: AgentMessage) -> Self {
        Self::Message(Box::new(message))
    }
}

impl From<Vec<AgentMessage>> for PromptInput {
    fn from(messages: Vec<AgentMessage>) -> Self {
        Self::Messages(messages)
    }
}

/// Upstream `normalizePromptInput` (agent.ts:406-423): a text prompt becomes
/// a single user message whose content is the text block plus any images.
fn normalize_prompt_input(input: PromptInput) -> Vec<AgentMessage> {
    match input {
        PromptInput::Messages(messages) => messages,
        PromptInput::Message(message) => vec![*message],
        PromptInput::Text { text, images } => {
            let content = if images.is_empty() {
                StringOrBlocks::Text(text)
            } else {
                let mut blocks = vec![TextOrImageBlock::Text(TextContent {
                    text,
                    text_signature: None,
                })];
                blocks.extend(images.into_iter().map(TextOrImageBlock::Image));
                StringOrBlocks::Blocks(blocks)
            };
            vec![AgentMessage::User(UserMessage {
                content,
                timestamp: now_ms(),
            })]
        }
    }
}

/// Upstream `createMutableAgentState` (agent.ts:81-110): seed the state from
/// [`AgentInitialState`]; `systemPrompt` and `tools` become the leading
/// system message unless `messages` already starts with one.
fn create_mutable_agent_state(initial: &AgentInitialState) -> AgentState {
    let mut state = AgentState {
        model: initial.model.clone().unwrap_or_else(unknown_model),
        thinking_level: initial.thinking_level.unwrap_or(ThinkingLevel::Off),
        tools: initial.tools.clone(),
        messages: initial.messages.clone(),
        ..AgentState::default()
    };
    let declarations: Vec<Tool> = state.tools.iter().map(|tool| tool.declaration()).collect();
    let initial_message = create_initial_system_message(
        initial.system_prompt.as_deref(),
        if declarations.is_empty() {
            None
        } else {
            Some(&declarations)
        },
    );
    if state.messages.first().map(|message| message.role()) != Some("system") {
        if let Some(message) = initial_message {
            state.messages.insert(0, AgentMessage::System(message));
        }
    }
    state
}

/// Upstream `processEvents` state reduction (agent.ts:559-597), applied
/// synchronously before the event's listeners are awaited.
fn reduce_state(state: &Mutex<AgentState>, event: &AgentEvent) {
    let mut state = state.lock().unwrap();
    match event.kind() {
        AgentEvent::MessageStart { message } | AgentEvent::MessageUpdate { message, .. } => {
            state.streaming_message = Some(message.clone());
        }
        AgentEvent::MessageEnd { message } => {
            state.streaming_message = None;
            state.messages.push(message.clone());
        }
        AgentEvent::ToolExecutionStart { tool_call_id, .. } => {
            state.pending_tool_calls.insert(tool_call_id.clone());
        }
        AgentEvent::ToolExecutionEnd { tool_call_id, .. } => {
            state.pending_tool_calls.remove(tool_call_id);
        }
        AgentEvent::TurnEnd { message, .. } => {
            if let AgentMessage::Assistant(assistant) = message {
                if let Some(error) = &assistant.error_message {
                    state.error_message = Some(error.clone());
                }
            }
        }
        AgentEvent::AgentEnd { .. } => {
            state.streaming_message = None;
        }
        AgentEvent::AgentStart | AgentEvent::TurnStart | AgentEvent::ToolExecutionUpdate { .. } => {
        }
        AgentEvent::Preserved(_) => unreachable!("kind() unwraps ingress"),
    }
}

/// Which low-level loop a run drives.
enum LoopKind {
    /// Upstream `runAgentLoop` with new prompt messages.
    Prompt(Vec<AgentMessage>),
    /// Upstream `runAgentLoopContinue` from the current transcript.
    Continue,
}

/// Stateful wrapper around the low-level agent loop (upstream `Agent`,
/// agent.ts:188-607).
///
/// `Agent` owns the current transcript, emits lifecycle events, executes
/// tools, and exposes queueing APIs for steering and follow-up messages.
pub struct Agent {
    state: Arc<Mutex<AgentState>>,
    listeners: ListenerList,
    next_listener_id: AtomicU64,
    steering_queue: Arc<Mutex<PendingMessageQueue>>,
    follow_up_queue: Arc<Mutex<PendingMessageQueue>>,
    active_run: Mutex<Option<ActiveRun>>,
    runtime: Mutex<AgentRuntimeOptions>,

    /// LLM-facing request executor (upstream public `streamFunction` field;
    /// the port resolves providers and credentials through the collection).
    pub models: Arc<Models>,
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("is_streaming", &self.state.lock().unwrap().is_streaming)
            .finish_non_exhaustive()
    }
}

impl Agent {
    /// Upstream `constructor(options)` (agent.ts:231-253). The port's
    /// default executor is the supplied [`Models`] collection; `stream_fn`
    /// overrides it without requiring a provider in that collection.
    pub fn new(options: AgentOptions, models: Arc<Models>) -> Self {
        let AgentOptions {
            initial_state,
            stream_fn,
            get_api_key,
            callbacks,
            transport,
            convert_to_llm,
            transform_context,
            before_tool_call,
            after_tool_call,
            should_stop_after_turn,
            prepare_next_turn,
            steering_mode,
            follow_up_mode,
            session_id,
            thinking_budgets,
            tool_execution,
            max_retry_delay_ms,
        } = options;
        let state = create_mutable_agent_state(&initial_state);
        Self {
            state: Arc::new(Mutex::new(state)),
            listeners: Arc::new(Mutex::new(Vec::new())),
            next_listener_id: AtomicU64::new(0),
            steering_queue: Arc::new(Mutex::new(PendingMessageQueue::new(
                steering_mode.unwrap_or(QueueMode::DEFAULT),
            ))),
            follow_up_queue: Arc::new(Mutex::new(PendingMessageQueue::new(
                follow_up_mode.unwrap_or(QueueMode::DEFAULT),
            ))),
            active_run: Mutex::new(None),
            runtime: Mutex::new(AgentRuntimeOptions {
                stream_fn,
                get_api_key,
                callbacks,
                transport: transport.unwrap_or(Transport::Auto),
                convert_to_llm: convert_to_llm.unwrap_or_else(|| Arc::new(default_convert_to_llm)),
                transform_context,
                before_tool_call,
                after_tool_call,
                should_stop_after_turn,
                prepare_next_turn,
                session_id,
                thinking_budgets,
                max_retry_delay_ms,
                tool_execution: tool_execution.unwrap_or(ToolExecutionMode::DEFAULT),
            }),
            models,
        }
    }

    /// Subscribe to agent lifecycle events (agent.ts:255-268).
    ///
    /// Listener futures are awaited in subscription order and are included in
    /// the current run's settlement. Listeners also receive the active abort
    /// token for the current run.
    ///
    /// `agent_end` is the final emitted event for a run, but the agent does
    /// not become idle until all awaited listeners for that event have
    /// settled.
    pub fn subscribe<F>(&self, listener: F) -> Unsubscribe
    where
        F: Fn(AgentEvent, CancellationToken) -> BoxFuture<'static, ()> + Send + Sync + 'static,
    {
        let id = self.next_listener_id.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut listeners) = self.listeners.lock() {
            listeners.push((id, Arc::new(listener)));
        }
        Unsubscribe {
            listeners: Arc::clone(&self.listeners),
            id,
        }
    }

    /// Current agent state (upstream `get state`, agent.ts:274-277).
    ///
    /// The guard allows the upstream mutator workflows — assigning `tools`
    /// or `messages` replaces the top-level collection, single fields can be
    /// patched in place.
    pub fn state(&self) -> MutexGuard<'_, AgentState> {
        self.state.lock().unwrap()
    }

    /// Per-run options (upstream's public mutable fields: `convertToLlm`,
    /// `transformContext`, the tool hooks, `sessionId`, `thinkingBudgets`,
    /// `maxRetryDelayMs`, `toolExecution`). Read at the start of each run;
    /// mutate between runs.
    pub fn runtime(&self) -> MutexGuard<'_, AgentRuntimeOptions> {
        self.runtime.lock().unwrap()
    }

    /// Controls how queued steering messages are drained (agent.ts:279-286).
    pub fn steering_mode(&self) -> QueueMode {
        self.steering_queue.lock().unwrap().mode()
    }

    /// Controls how queued steering messages are drained (agent.ts:279-282).
    pub fn set_steering_mode(&self, mode: QueueMode) {
        self.steering_queue.lock().unwrap().set_mode(mode);
    }

    /// Controls how queued follow-up messages are drained (agent.ts:288-295).
    pub fn follow_up_mode(&self) -> QueueMode {
        self.follow_up_queue.lock().unwrap().mode()
    }

    /// Controls how queued follow-up messages are drained (agent.ts:288-291).
    pub fn set_follow_up_mode(&self, mode: QueueMode) {
        self.follow_up_queue.lock().unwrap().set_mode(mode);
    }

    /// Queue a message to be injected after the current assistant turn
    /// finishes (agent.ts:297-300).
    pub fn steer(&self, message: AgentMessage) {
        self.steering_queue.lock().unwrap().enqueue(message);
    }

    /// Queue a message to run only after the agent would otherwise stop
    /// (agent.ts:302-305).
    pub fn follow_up(&self, message: AgentMessage) {
        self.follow_up_queue.lock().unwrap().enqueue(message);
    }

    /// Remove all queued steering messages (agent.ts:307-310).
    pub fn clear_steering_queue(&self) {
        self.steering_queue.lock().unwrap().clear();
    }

    /// Remove all queued follow-up messages (agent.ts:312-315).
    pub fn clear_follow_up_queue(&self) {
        self.follow_up_queue.lock().unwrap().clear();
    }

    /// Remove all queued steering and follow-up messages (agent.ts:317-321).
    pub fn clear_all_queues(&self) {
        self.clear_steering_queue();
        self.clear_follow_up_queue();
    }

    /// Returns true when either queue still contains pending messages
    /// (agent.ts:323-326).
    pub fn has_queued_messages(&self) -> bool {
        self.steering_queue.lock().unwrap().has_items()
            || self.follow_up_queue.lock().unwrap().has_items()
    }

    /// Active abort token for the current run, if any (agent.ts:328-331).
    pub fn signal(&self) -> Option<CancellationToken> {
        self.active_run
            .lock()
            .unwrap()
            .as_ref()
            .map(|run| run.token.clone())
    }

    /// Abort the current run, if one is active (agent.ts:333-336).
    pub fn abort(&self) {
        if let Some(run) = self.active_run.lock().unwrap().as_ref() {
            run.token.cancel();
        }
    }

    /// Resolve when the current run and all awaited event listeners have
    /// finished (agent.ts:338-345). Resolves after `agent_end` listeners
    /// settle; resolves immediately when idle.
    pub async fn wait_for_idle(&self) {
        let mut idle = match self.active_run.lock().unwrap().as_ref() {
            Some(run) => run.idle.subscribe(),
            None => return,
        };
        while !*idle.borrow() {
            if idle.changed().await.is_err() {
                return;
            }
        }
    }

    /// Clear conversation state and queues while retaining the replayed
    /// prompt/tool baseline (agent.ts:347-361). Errors while a run is active
    /// (upstream throws).
    pub fn reset(&self) -> anyhow::Result<()> {
        if self.active_run.lock().unwrap().is_some() {
            bail!("Agent is already processing. Wait for completion before resetting.");
        }
        let mut state = self.state.lock().unwrap();
        let llm_messages: Vec<Message> = state
            .messages
            .iter()
            .filter_map(AgentMessage::to_message)
            .collect();
        state.messages = get_current_system_message(&llm_messages)
            .map(|message| vec![AgentMessage::System(message)])
            .unwrap_or_default();
        state.is_streaming = false;
        state.streaming_message = None;
        state.pending_tool_calls.clear();
        state.error_message = None;
        drop(state);
        self.clear_follow_up_queue();
        self.clear_steering_queue();
        Ok(())
    }

    /// Start a new prompt from text (+ optional images), a single message,
    /// or a batch of messages (agent.ts:363-374). Errors when the agent is
    /// already processing (upstream throws).
    pub async fn prompt(&self, input: impl Into<PromptInput>) -> anyhow::Result<()> {
        if self.active_run.lock().unwrap().is_some() {
            bail!(
                "Agent is already processing a prompt. Use steer() or followUp() to queue \
                 messages, or wait for completion."
            );
        }
        let messages = normalize_prompt_input(input.into());
        self.run_prompt_messages(messages, false).await
    }

    /// Continue from the current transcript (agent.ts:376-404). The last
    /// message must be a user or tool-result message; from an assistant tail
    /// the queued steering — else follow-up — messages are processed
    /// instead. Errors when the agent is already processing (upstream
    /// throws).
    pub async fn continue_run(&self) -> anyhow::Result<()> {
        if self.active_run.lock().unwrap().is_some() {
            bail!("Agent is already processing. Wait for completion before continuing.");
        }

        let messages = self.state.lock().unwrap().messages.clone();
        let last_role = messages.last().map(|message| message.role().to_string());
        let all_system = messages.iter().all(|message| message.role() == "system");
        let Some(last_role) = last_role else {
            bail!("No messages to continue from");
        };
        if all_system {
            bail!("No messages to continue from");
        }

        if last_role == "assistant" {
            let queued_steering = self.steering_queue.lock().unwrap().drain();
            if !queued_steering.is_empty() {
                return self.run_prompt_messages(queued_steering, true).await;
            }

            let queued_follow_ups = self.follow_up_queue.lock().unwrap().drain();
            if !queued_follow_ups.is_empty() {
                return self.run_prompt_messages(queued_follow_ups, false).await;
            }

            bail!("Cannot continue from message role: assistant");
        }

        self.run_with_lifecycle(LoopKind::Continue, false).await
    }

    /// Upstream `runPromptMessages` (agent.ts:425-439).
    async fn run_prompt_messages(
        &self,
        messages: Vec<AgentMessage>,
        skip_initial_steering_poll: bool,
    ) -> anyhow::Result<()> {
        self.run_with_lifecycle(LoopKind::Prompt(messages), skip_initial_steering_poll)
            .await
    }

    /// Upstream `runWithLifecycle` (agent.ts:501-524): register the run,
    /// mark the state streaming, drive the loop, settle run failures with
    /// the synthetic failure-message choreography, then clear the runtime
    /// state and release waiters. A [`MaxTurnsExceeded`] error is the one
    /// exception: the loop settled the run itself (its `agent_end` already
    /// emitted), so nothing is re-emitted.
    async fn run_with_lifecycle(
        &self,
        run: LoopKind,
        skip_initial_steering_poll: bool,
    ) -> anyhow::Result<()> {
        let token = {
            let mut active = self.active_run.lock().unwrap();
            if active.is_some() {
                bail!("Agent is already processing.");
            }
            let token = CancellationToken::new();
            let (idle, _) = watch::channel(false);
            *active = Some(ActiveRun {
                token: token.clone(),
                idle,
            });
            token
        };

        {
            let mut state = self.state.lock().unwrap();
            state.is_streaming = true;
            state.streaming_message = None;
            state.error_message = None;
        }

        let sink = self.create_sink(&token);
        let result = match run {
            LoopKind::Prompt(messages) => {
                run_agent_loop(
                    messages,
                    self.create_context_snapshot(),
                    self.create_loop_config(skip_initial_steering_poll),
                    self.models.as_ref(),
                    Some(token.clone()),
                    &sink,
                )
                .await
            }
            LoopKind::Continue => {
                run_agent_loop_continue(
                    self.create_context_snapshot(),
                    self.create_loop_config(false),
                    self.models.as_ref(),
                    Some(token.clone()),
                    &sink,
                )
                .await
            }
        };

        if let Err(error) = result {
            // The max-turns guard settles its run inside the loop: the single
            // `agent_end` was already emitted, so the failure choreography
            // must not run — a second `agent_end` breaks upstream's
            // one-agent_end-per-run invariant and a synthetic failure message
            // would be appended after the run already ended.
            if error.downcast_ref::<MaxTurnsExceeded>().is_none() {
                let aborted = token.is_cancelled();
                self.handle_run_failure(error, aborted, &sink).await;
            }
        }
        self.finish_run();
        Ok(())
    }

    /// Upstream `handleRunFailure` (agent.ts:526-542): settle a thrown run
    /// with the full synthetic lifecycle — a failed assistant message
    /// carried through `message_start`/`message_end`/`turn_end`/`agent_end`.
    async fn handle_run_failure(&self, error: anyhow::Error, aborted: bool, sink: &AgentEventSink) {
        let failure_message = {
            let state = self.state.lock().unwrap();
            AgentMessage::Assistant(AssistantMessage {
                content: vec![AssistantBlock::Text(TextContent {
                    text: String::new(),
                    text_signature: None,
                })],
                api: state.model.api.clone(),
                provider: state.model.provider.clone(),
                model: state.model.id.clone(),
                response_model: None,
                response_id: None,
                provider_thinking_level: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: if aborted {
                    StopReason::Aborted
                } else {
                    StopReason::Error
                },
                deferred: None,
                error_message: Some(error.to_string()),
                raw_stop_reason: None,
                end_turn: None,
                timestamp: now_ms(),
            })
        };
        (sink)(AgentEvent::MessageStart {
            message: failure_message.clone(),
        })
        .await;
        (sink)(AgentEvent::MessageEnd {
            message: failure_message.clone(),
        })
        .await;
        (sink)(AgentEvent::TurnEnd {
            message: failure_message.clone(),
            tool_results: Vec::new(),
        })
        .await;
        (sink)(AgentEvent::AgentEnd {
            messages: vec![failure_message],
        })
        .await;
    }

    /// Upstream `finishRun` (agent.ts:544-550): clear the runtime-owned
    /// state, resolve the run promise (`wait_for_idle` waiters), unregister
    /// the run.
    fn finish_run(&self) {
        {
            let mut state = self.state.lock().unwrap();
            state.is_streaming = false;
            state.streaming_message = None;
            state.pending_tool_calls.clear();
        }
        if let Some(run) = self.active_run.lock().unwrap().take() {
            let _ = run.idle.send(true);
        }
    }

    /// The loop's awaited emit sink: reduce the runtime state synchronously,
    /// then await the listeners in registration order (upstream
    /// `processEvents`, agent.ts:552-606). Because the loop awaits this sink
    /// at every emission, a listener gate on `message_end` is a barrier
    /// before tool preflight (README "Agent" section).
    fn create_sink(&self, token: &CancellationToken) -> AgentEventSink {
        let state = Arc::clone(&self.state);
        let listeners = Arc::clone(&self.listeners);
        let token = token.clone();
        Arc::new(move |event| {
            reduce_state(&state, &event);
            let listeners = Arc::clone(&listeners);
            let token = token.clone();
            Box::pin(async move {
                // Index-based iteration over the live list, so a listener
                // subscribed during dispatch is still visited this event
                // (upstream iterates the Set live).
                let mut index = 0usize;
                loop {
                    let listener = listeners
                        .lock()
                        .ok()
                        .and_then(|list| list.get(index).map(|(_, listener)| Arc::clone(listener)));
                    let Some(listener) = listener else {
                        break;
                    };
                    (listener)(event.clone(), token.clone()).await;
                    index += 1;
                }
            })
        })
    }

    /// Upstream `createContextSnapshot` (agent.ts:453-458).
    fn create_context_snapshot(&self) -> AgentContext {
        let state = self.state.lock().unwrap();
        AgentContext {
            messages: state.messages.clone(),
            tools: state.tools.clone(),
        }
    }

    /// Upstream `createLoopConfig` (agent.ts:460-499): bridge the Agent's
    /// runtime options into the low-level loop config, backing the loop's
    /// queue hooks with the steering/follow-up queues.
    fn create_loop_config(&self, skip_initial_steering_poll: bool) -> AgentLoopConfig {
        let (model, thinking_level) = {
            let state = self.state.lock().unwrap();
            (state.model.clone(), state.thinking_level)
        };
        let runtime = self.runtime.lock().unwrap().clone();
        let mut config = AgentLoopConfig::new(model, Arc::clone(&runtime.convert_to_llm));
        config.stream_fn = runtime.stream_fn.clone();
        config.get_api_key = runtime.get_api_key.clone();
        config.stream_options.stream.callbacks = runtime.callbacks.clone();
        config.stream_options.stream.transport = Some(runtime.transport);
        config.thinking_level = (thinking_level != ThinkingLevel::Off).then_some(thinking_level);
        config.transform_context = runtime.transform_context.clone();
        config.tool_execution = Some(runtime.tool_execution);
        config.session_id = runtime.session_id.clone();
        config.thinking_budgets = runtime.thinking_budgets;
        config.max_retry_delay_ms = runtime.max_retry_delay_ms;
        config.before_tool_call = runtime.before_tool_call.clone();
        config.after_tool_call = runtime.after_tool_call.clone();
        config.should_stop_after_turn = runtime.should_stop_after_turn.clone();
        config.prepare_next_turn = runtime.prepare_next_turn.clone();

        // Upstream `getSteeringMessages` (agent.ts:490-496): the first poll
        // is skipped when the run's messages ARE the drained steering queue
        // (continue from an assistant tail), so one-at-a-time semantics keep
        // exactly one message per drain point.
        let steering_queue = Arc::clone(&self.steering_queue);
        let skip = Arc::new(AtomicBool::new(skip_initial_steering_poll));
        config.get_steering_messages = Some(Arc::new(move || {
            let skip_this_poll = skip.swap(false, Ordering::SeqCst);
            let drained = if skip_this_poll {
                Vec::new()
            } else {
                steering_queue.lock().unwrap().drain()
            };
            Box::pin(async move { drained })
        }));
        let follow_up_queue = Arc::clone(&self.follow_up_queue);
        config.get_follow_up_messages = Some(Arc::new(move || {
            let drained = follow_up_queue.lock().unwrap().drain();
            Box::pin(async move { drained })
        }));
        config
    }
}

#[cfg(test)]
#[path = "stream_adapter_tests.rs"]
mod stream_adapter_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core::agent_loop::{
        BeforeToolCallContext, BeforeToolCallHook, BeforeToolCallOutcome, PrepareNextTurnContext,
        ShouldStopAfterTurnContext,
    };
    use crate::agent_core::types::{AgentTool, AgentToolResult, AgentToolUpdateCallback};
    use crate::ai::models::faux::{FauxResponseFactory, FauxTokenSize};
    use crate::ai::models::{
        create_models, faux_assistant_message, faux_provider, faux_tool_call, CreateModelsOptions,
        FauxFactoryArgs, FauxMessageOptions, FauxProviderHandle, FauxProviderOptions,
        FauxResponseStep, FauxToolCallOptions,
    };
    use crate::ai::transcript::content_text;
    use crate::ai::types::message::{Sections, SystemMessage};
    use crate::ai::types::tool::ToolReference;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;
    use tokio::sync::Notify;

    const TS: i64 = 1758240000000;

    // ---- fixtures (agent.test.ts:21-99) ----

    fn faux_models() -> (Arc<Models>, FauxProviderHandle, crate::ai::types::Model) {
        let faux = faux_provider(FauxProviderOptions::default());
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(faux.provider.clone());
        let model = faux.get_model(None).expect("faux default model");
        (Arc::new(models), faux, model)
    }

    /// These legacy tests exercise the default [`Models`] stream path, which
    /// routes by model provider. Seed the faux model so that its provider
    /// serves the response (the upstream DEFAULT_MODEL still applies for
    /// agents that never stream). Custom `stream_fn` injection is covered
    /// separately by the stream adapter tests.
    fn faux_options(model: &crate::ai::types::Model) -> AgentOptions {
        AgentOptions {
            initial_state: AgentInitialState {
                model: Some(model.clone()),
                ..AgentInitialState::default()
            },
            ..AgentOptions::default()
        }
    }

    fn text_block(text: &str) -> crate::ai::types::message::TextOrImageBlock {
        crate::ai::types::message::TextOrImageBlock::Text(TextContent {
            text: text.to_string(),
            text_signature: None,
        })
    }

    fn user_message(text: &str, timestamp: i64) -> AgentMessage {
        AgentMessage::User(UserMessage {
            content: StringOrBlocks::Text(text.to_string()),
            timestamp,
        })
    }

    fn assistant_message(text: &str) -> AgentMessage {
        AgentMessage::Assistant(faux_assistant_message(text, FauxMessageOptions::default()))
    }

    /// Upstream `createTool` (agent.test.ts:56-64): a no-argument tool whose
    /// result text is its own name.
    fn create_tool(name: &str) -> AgentTool {
        let owned = name.to_string();
        AgentTool {
            name: owned.clone(),
            label: owned.clone(),
            description: format!("{owned} tool"),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            execute: Arc::new(
                move |_id: String,
                      _params: serde_json::Value,
                      _signal: Option<CancellationToken>,
                      _on_update: Option<Arc<AgentToolUpdateCallback>>| {
                    let name = owned.clone();
                    Box::pin(async move {
                        Ok(AgentToolResult {
                            content: vec![text_block(&name)],
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

    fn text_response(text: &str) -> FauxResponseStep {
        faux_assistant_message(text, FauxMessageOptions::default()).into()
    }

    fn tool_call_response(
        calls: Vec<(&str, &str, serde_json::Value)>,
        stop_reason: StopReason,
    ) -> FauxResponseStep {
        let blocks: Vec<crate::ai::types::message::AssistantBlock> = calls
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

    fn recording_listener(
        events: Arc<Mutex<Vec<AgentEvent>>>,
    ) -> impl Fn(AgentEvent, CancellationToken) -> BoxFuture<'static, ()> + Send + Sync {
        move |event: AgentEvent, _signal: CancellationToken| {
            let events = events.clone();
            Box::pin(async move {
                events.lock().unwrap().push(event);
            })
        }
    }

    fn role_names(state: &AgentState) -> Vec<&str> {
        state
            .messages
            .iter()
            .map(|message| message.role())
            .collect()
    }

    /// A factory step that notifies `started`, blocks until `release`, then
    /// responds — the port of the upstream blocking mock streams.
    fn blocking_factory_step(
        started: Arc<Notify>,
        release: Arc<Notify>,
        text: &str,
    ) -> FauxResponseStep {
        let text = text.to_string();
        FauxResponseStep::Factory(Arc::new(move |_args: FauxFactoryArgs| {
            let (started, release, text) = (started.clone(), release.clone(), text.clone());
            Box::pin(async move {
                started.notify_one();
                release.notified().await;
                Ok(faux_assistant_message(text, FauxMessageOptions::default()))
            })
        }))
    }

    // ---- construction (agent.test.ts:123-171) ----

    /// Oracle "should create an agent instance with default state".
    #[test]
    fn creates_an_agent_with_default_state() {
        let (models, _faux, _model) = faux_models();
        let agent = Agent::new(AgentOptions::default(), models);
        let state = agent.state();
        assert_eq!(state.model.id, "unknown");
        assert_eq!(state.thinking_level, ThinkingLevel::Off);
        assert!(state.tools.is_empty());
        assert!(state.messages.is_empty());
        assert!(!state.is_streaming);
        assert_eq!(state.streaming_message, None);
        assert!(state.pending_tool_calls.is_empty());
        assert_eq!(state.error_message, None);
    }

    /// Oracle "should create an agent instance with custom initial state"
    /// (the upstream `getModel("openai", "gpt-4o-mini")` becomes the faux
    /// provider's model: the port has no global model registry).
    #[test]
    fn creates_an_agent_with_custom_initial_state() {
        let (models, faux, _model) = faux_models();
        let custom_model = faux.get_model(None).expect("faux model");
        let agent = Agent::new(
            AgentOptions {
                initial_state: AgentInitialState {
                    system_prompt: Some("You are a helpful assistant.".into()),
                    model: Some(custom_model.clone()),
                    thinking_level: Some(ThinkingLevel::Low),
                    ..AgentInitialState::default()
                },
                ..AgentOptions::default()
            },
            models,
        );
        let state = agent.state();
        assert_eq!(
            state.messages,
            vec![AgentMessage::System(SystemMessage {
                content: StringOrBlocks::Text("You are a helpful assistant.".into()),
                sections: None,
                tools_added: None,
                tools_removed: None,
                timestamp: 0,
            })]
        );
        assert_eq!(state.model, custom_model);
        assert_eq!(state.thinking_level, ThinkingLevel::Low);
    }

    /// Oracle "converts initial prompt and tools into transcript state".
    #[test]
    fn converts_initial_prompt_and_tools_into_transcript_state() {
        let (models, _faux, _model) = faux_models();
        let agent = Agent::new(
            AgentOptions {
                initial_state: AgentInitialState {
                    system_prompt: Some("You are helpful.".into()),
                    tools: vec![Arc::new(create_tool("echo"))],
                    ..AgentInitialState::default()
                },
                ..AgentOptions::default()
            },
            models,
        );
        let state = agent.state();
        assert_eq!(state.messages.len(), 1);
        let AgentMessage::System(initial) = &state.messages[0] else {
            panic!("expected initial system message");
        };
        assert_eq!(content_text(&initial.content), "You are helpful.");
        let added = initial.tools_added.as_ref().expect("toolsAdded");
        assert_eq!(
            added
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["echo"]
        );
    }

    // ---- tool loadout declarations (agent.test.ts:173-289) ----

    /// Oracle "declares tool loadout changes to the model before the next
    /// request": each request's system messages are recorded from the
    /// provider context.
    #[tokio::test]
    async fn declares_tool_loadout_changes_to_the_model_before_the_next_request() {
        let (models, faux, model) = faux_models();
        let requests = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
        let factory: FauxResponseFactory = {
            let requests = requests.clone();
            Arc::new(move |args: FauxFactoryArgs| {
                let requests = requests.clone();
                Box::pin(async move {
                    let mut record: Vec<String> = Vec::new();
                    for message in args.context.messages() {
                        if let Message::System(system) = message {
                            let added = system
                                .tools_added
                                .as_ref()
                                .map(|tools| {
                                    tools
                                        .iter()
                                        .map(|tool| tool.name.clone())
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default();
                            let removed = system
                                .tools_removed
                                .as_ref()
                                .map(|tools| {
                                    tools
                                        .iter()
                                        .map(|tool| tool.name.clone())
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default();
                            record.push(format!("+{}", added.join(",")));
                            record.push(format!("-{}", removed.join(",")));
                        }
                    }
                    requests.lock().unwrap().push(record);
                    Ok(faux_assistant_message(
                        "done",
                        FauxMessageOptions::default(),
                    ))
                }) as BoxFuture<'static, Result<AssistantMessage, String>>
            })
        };
        faux.set_responses(vec![
            FauxResponseStep::Factory(factory.clone()),
            FauxResponseStep::Factory(factory.clone()),
            FauxResponseStep::Factory(factory),
        ]);

        let agent = Arc::new(Agent::new(
            AgentOptions {
                initial_state: AgentInitialState {
                    system_prompt: Some("You are helpful.".into()),
                    tools: vec![Arc::new(create_tool("first"))],
                    model: Some(model.clone()),
                    ..AgentInitialState::default()
                },
                ..AgentOptions::default()
            },
            models,
        ));

        agent.prompt("one").await.unwrap();
        agent.state().tools = vec![Arc::new(create_tool("second"))];
        agent.prompt("two").await.unwrap();
        agent.prompt("three").await.unwrap();

        assert_eq!(
            *requests.lock().unwrap(),
            vec![
                vec!["+first".to_string(), "-".to_string()],
                vec![
                    "+first".to_string(),
                    "-".to_string(),
                    "+second".to_string(),
                    "-first".to_string()
                ],
                vec![
                    "+first".to_string(),
                    "-".to_string(),
                    "+second".to_string(),
                    "-first".to_string()
                ],
            ]
        );

        let state = agent.state();
        let update = state
            .messages
            .iter()
            .find_map(|message| match message {
                AgentMessage::System(system) if system.tools_removed.is_some() => {
                    Some(system.clone())
                }
                _ => None,
            })
            .expect("loadout update message");
        assert_eq!(update.content, StringOrBlocks::Text(String::new()));
        let added = update.tools_added.as_ref().expect("toolsAdded");
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].name, "second");
        assert_eq!(added[0].description, "second tool");
        assert_eq!(
            update.tools_removed.as_ref().expect("toolsRemoved"),
            &vec![ToolReference {
                name: "first".into()
            }]
        );

        // The initial system message carries declarations only, never
        // executors (upstream: toolsAdded[0] has no `execute` property).
        let AgentMessage::System(initial) = &state.messages[0] else {
            panic!("expected initial system message");
        };
        let declaration = &initial.tools_added.as_ref().expect("toolsAdded")[0];
        let wire = serde_json::to_value(declaration).unwrap();
        assert!(wire.get("execute").is_none());
        assert_eq!(wire["name"], serde_json::json!("first"));
    }

    /// Oracle "merges tool changes into a pending system message".
    #[tokio::test]
    async fn merges_tool_changes_into_a_pending_system_message() {
        let (models, faux, model) = faux_models();
        let system_count = Arc::new(Mutex::new(0usize));
        let counter = system_count.clone();
        faux.set_responses(vec![FauxResponseStep::Factory(Arc::new(
            move |args: FauxFactoryArgs| {
                let counter = counter.clone();
                Box::pin(async move {
                    *counter.lock().unwrap() = args
                        .context
                        .messages()
                        .iter()
                        .filter(|message| matches!(message, Message::System(_)))
                        .count();
                    Ok(faux_assistant_message(
                        "done",
                        FauxMessageOptions::default(),
                    ))
                })
            },
        ))]);

        let agent = Arc::new(Agent::new(
            AgentOptions {
                initial_state: AgentInitialState {
                    system_prompt: Some("You are helpful.".into()),
                    model: Some(model.clone()),
                    ..AgentInitialState::default()
                },
                ..AgentOptions::default()
            },
            models,
        ));
        agent.state().tools = vec![Arc::new(create_tool("echo"))];
        agent
            .prompt(vec![
                AgentMessage::System(SystemMessage {
                    content: StringOrBlocks::Text(String::new()),
                    sections: Some(Sections::new(vec![(
                        "skills".into(),
                        Some("<skills>x</skills>".into()),
                    )])),
                    tools_added: None,
                    tools_removed: None,
                    timestamp: 1,
                }),
                user_message("hi", 2),
            ])
            .await
            .unwrap();

        assert_eq!(*system_count.lock().unwrap(), 2);
        let state = agent.state();
        let AgentMessage::System(pending) = &state.messages[1] else {
            panic!("expected pending system message");
        };
        assert_eq!(pending.content, StringOrBlocks::Text(String::new()));
        assert_eq!(
            pending.sections,
            Some(Sections::new(vec![(
                "skills".into(),
                Some("<skills>x</skills>".into())
            )]))
        );
        let added = pending.tools_added.as_ref().expect("toolsAdded");
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].name, "echo");
        assert_eq!(added[0].description, "echo tool");
        assert_eq!(pending.timestamp, 1);
    }

    /// Oracle "rewrites pending tool declarations to match the executable
    /// set": the executable set wins over a pending message's declarations.
    #[tokio::test]
    async fn rewrites_pending_tool_declarations_to_match_the_executable_set() {
        let (models, faux, model) = faux_models();
        faux.set_responses(vec![text_response("done")]);
        let agent = Arc::new(Agent::new(
            AgentOptions {
                initial_state: AgentInitialState {
                    system_prompt: Some("You are helpful.".into()),
                    tools: vec![Arc::new(create_tool("first"))],
                    model: Some(model.clone()),
                    ..AgentInitialState::default()
                },
                ..AgentOptions::default()
            },
            models,
        ));

        agent
            .prompt(vec![
                AgentMessage::System(SystemMessage {
                    content: StringOrBlocks::Text(String::new()),
                    sections: Some(Sections::new(vec![(
                        "note".into(),
                        Some("<note>x</note>".into()),
                    )])),
                    tools_added: Some(vec![create_tool("second").declaration()]),
                    tools_removed: Some(vec![ToolReference {
                        name: "first".into(),
                    }]),
                    timestamp: 1,
                }),
                user_message("hi", 2),
            ])
            .await
            .unwrap();

        let state = agent.state();
        let AgentMessage::System(pending) = &state.messages[1] else {
            panic!("expected pending system message");
        };
        assert_eq!(pending.content, StringOrBlocks::Text(String::new()));
        assert_eq!(pending.tools_added, None);
        assert_eq!(pending.tools_removed, None);
        assert_eq!(
            pending.sections,
            Some(Sections::new(vec![(
                "note".into(),
                Some("<note>x</note>".into())
            )]))
        );
        assert_eq!(pending.timestamp, 1);

        // The replayed system message still declares the executable set.
        let llm_messages: Vec<Message> = state
            .messages
            .iter()
            .filter_map(AgentMessage::to_message)
            .collect();
        let current = get_current_system_message(&llm_messages).expect("current system message");
        assert_eq!(
            current.tools_added.as_ref().map(|tools| tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>()),
            Some(vec!["first"])
        );
    }

    // ---- reset (agent.test.ts:291-316) ----

    /// Oracle "restores the transcript baseline when reset".
    #[test]
    fn restores_the_transcript_baseline_when_reset() {
        let (models, _faux, _model) = faux_models();
        let agent = Agent::new(
            AgentOptions {
                initial_state: AgentInitialState {
                    system_prompt: Some("You are helpful.".into()),
                    tools: vec![Arc::new(create_tool("echo"))],
                    messages: vec![user_message("old", 1)],
                    ..AgentInitialState::default()
                },
                ..AgentOptions::default()
            },
            models,
        );

        agent.reset().unwrap();

        let state = agent.state();
        assert_eq!(state.messages.len(), 1);
        let AgentMessage::System(initial) = &state.messages[0] else {
            panic!("expected initial system message");
        };
        assert_eq!(content_text(&initial.content), "You are helpful.");
        assert_eq!(
            initial
                .tools_added
                .as_ref()
                .expect("toolsAdded")
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["echo"]
        );
    }

    // ---- subscription (agent.test.ts:318-338) ----

    /// Oracle "should subscribe to events": no initial event, mutators emit
    /// nothing, unsubscribe stops delivery.
    #[test]
    fn subscribe_receives_no_initial_events_and_unsubscribe_stops_delivery() {
        let (models, _faux, _model) = faux_models();
        let agent = Agent::new(AgentOptions::default(), models);

        let count = Arc::new(AtomicUsize::new(0));
        let unsubscribe = {
            let count = count.clone();
            agent.subscribe(move |_event: AgentEvent, _signal: CancellationToken| {
                let count = count.clone();
                Box::pin(async move {
                    count.fetch_add(1, Ordering::SeqCst);
                })
            })
        };
        assert_eq!(count.load(Ordering::SeqCst), 0);

        agent.state().thinking_level = ThinkingLevel::Low;
        assert_eq!(agent.state().thinking_level, ThinkingLevel::Low);
        assert_eq!(count.load(Ordering::SeqCst), 0);

        unsubscribe.unsubscribe();
        agent.state().thinking_level = ThinkingLevel::High;
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

    // ---- run failure lifecycle (agent.test.ts:340-369) ----

    /// Oracle "emits full lifecycle events for thrown run failures": the
    /// port's provider failure (a response factory that errors) settles the
    /// run with the same synthetic choreography.
    #[tokio::test]
    async fn emits_full_lifecycle_events_for_failed_runs() {
        let (models, faux, model) = faux_models();
        faux.set_responses(vec![FauxResponseStep::Factory(Arc::new(
            |_args: FauxFactoryArgs| {
                Box::pin(async move { Err("provider exploded".to_string()) })
                    as BoxFuture<'static, Result<AssistantMessage, String>>
            },
        ))]);
        let agent = Arc::new(Agent::new(faux_options(&model), models));
        let events = Arc::new(Mutex::new(Vec::<AgentEvent>::new()));
        agent.subscribe(recording_listener(events.clone()));

        agent.prompt("hello").await.unwrap();

        let names: Vec<&str> = events
            .lock()
            .unwrap()
            .iter()
            .map(|event| event_name(event))
            .collect();
        assert_eq!(
            names,
            [
                "agent_start",
                "turn_start",
                "message_start",
                "message_end",
                "message_start",
                "message_end",
                "turn_end",
                "agent_end",
            ]
        );
        let state = agent.state();
        let Some(AgentMessage::Assistant(last)) = state.messages.last() else {
            panic!("expected trailing assistant message");
        };
        assert_eq!(last.stop_reason, StopReason::Error);
        assert_eq!(last.error_message.as_deref(), Some("provider exploded"));
        assert_eq!(state.error_message.as_deref(), Some("provider exploded"));
    }

    /// The M1-carried max-turns guard pins the one-agent_end-per-run
    /// invariant through the Agent (upstream agent-loop.ts emits a single
    /// agent_end on every path, agent-loop.ts:223/259/278; the guard is the
    /// port's only loop-level throw): the loop settles the run itself,
    /// `handle_run_failure` must not run, and no synthetic failure message
    /// is appended after the run ended.
    #[tokio::test]
    async fn max_turns_runs_emit_exactly_one_agent_end() {
        let (models, faux, model) = faux_models();
        let executed = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        // The Agent does not override the loop's default max_turns (25), so
        // drive 25 non-terminating tool turns to trip the guard.
        faux.set_responses(vec![
            tool_call_response(
                vec![("tool-1", "noop", serde_json::json!({}))],
                StopReason::ToolUse,
            );
            25
        ]);
        let agent = Arc::new(Agent::new(
            AgentOptions {
                initial_state: AgentInitialState {
                    tools: vec![Arc::new(echo_tool(executed.clone()))],
                    model: Some(model.clone()),
                    ..AgentInitialState::default()
                },
                ..AgentOptions::default()
            },
            models,
        ));
        let events = Arc::new(Mutex::new(Vec::<AgentEvent>::new()));
        agent.subscribe(recording_listener(events.clone()));

        // The run settles instead of surfacing as a failure.
        agent.prompt("start").await.unwrap();

        let events = events.lock().unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, AgentEvent::AgentEnd { .. }))
                .count(),
            1
        );
        assert!(matches!(events.last(), Some(AgentEvent::AgentEnd { .. })));
        drop(events);
        assert_eq!(agent.state().error_message, None);
        // No synthetic failure message: the transcript ends with turn 25's
        // toolResult, not a trailing assistant error message.
        assert_eq!(
            agent.state().messages.last().map(|message| message.role()),
            Some("toolResult")
        );
    }

    // ---- awaited subscribers (agent.test.ts:371-442) ----

    /// Oracle "should await async subscribers before prompt resolves".
    #[tokio::test]
    async fn awaits_async_subscribers_before_prompt_resolves() {
        let (models, faux, model) = faux_models();
        faux.set_responses(vec![text_response("ok")]);
        let agent = Arc::new(Agent::new(faux_options(&model), models));

        let (barrier_tx, barrier_rx) = tokio::sync::oneshot::channel::<()>();
        let barrier_rx = Arc::new(Mutex::new(Some(barrier_rx)));
        let listener_finished = Arc::new(AtomicBool::new(false));
        {
            let (barrier_rx, listener_finished) = (barrier_rx.clone(), listener_finished.clone());
            agent.subscribe(move |event: AgentEvent, _signal: CancellationToken| {
                let (barrier_rx, listener_finished) =
                    (barrier_rx.clone(), listener_finished.clone());
                Box::pin(async move {
                    if matches!(event, AgentEvent::AgentEnd { .. }) {
                        let rx = barrier_rx.lock().unwrap().take();
                        if let Some(rx) = rx {
                            let _ = rx.await;
                        }
                        listener_finished.store(true, Ordering::SeqCst);
                    }
                })
            });
        }

        let prompt_resolved = Arc::new(AtomicBool::new(false));
        let prompt_agent = agent.clone();
        let prompt_flag = prompt_resolved.clone();
        let prompt_task = tokio::spawn(async move {
            prompt_agent.prompt("hello").await.unwrap();
            prompt_flag.store(true, Ordering::SeqCst);
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!prompt_resolved.load(Ordering::SeqCst));
        assert!(!listener_finished.load(Ordering::SeqCst));
        assert!(agent.state().is_streaming);

        let _ = barrier_tx.send(());
        prompt_task.await.unwrap();

        assert!(listener_finished.load(Ordering::SeqCst));
        assert!(prompt_resolved.load(Ordering::SeqCst));
        assert!(!agent.state().is_streaming);
    }

    /// Oracle "waitForIdle should wait for async subscribers".
    #[tokio::test]
    async fn wait_for_idle_waits_for_async_subscribers() {
        let (models, faux, model) = faux_models();
        faux.set_responses(vec![text_response("ok")]);
        let agent = Arc::new(Agent::new(faux_options(&model), models));

        let (barrier_tx, barrier_rx) = tokio::sync::oneshot::channel::<()>();
        let barrier_rx = Arc::new(Mutex::new(Some(barrier_rx)));
        {
            let barrier_rx = barrier_rx.clone();
            agent.subscribe(move |event: AgentEvent, _signal: CancellationToken| {
                let barrier_rx = barrier_rx.clone();
                Box::pin(async move {
                    if let AgentEvent::MessageEnd {
                        message: AgentMessage::Assistant(_),
                    } = event
                    {
                        let rx = barrier_rx.lock().unwrap().take();
                        if let Some(rx) = rx {
                            let _ = rx.await;
                        }
                    }
                })
            });
        }

        let prompt_agent = agent.clone();
        let prompt_task = tokio::spawn(async move { prompt_agent.prompt("hello").await });

        let idle_resolved = Arc::new(AtomicBool::new(false));
        let idle_agent = agent.clone();
        let idle_flag = idle_resolved.clone();
        let idle_task = tokio::spawn(async move {
            idle_agent.wait_for_idle().await;
            idle_flag.store(true, Ordering::SeqCst);
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!idle_resolved.load(Ordering::SeqCst));
        assert!(agent.state().is_streaming);

        let _ = barrier_tx.send(());
        let (prompt_result, _) = tokio::join!(prompt_task, idle_task);
        prompt_result.unwrap().unwrap();

        assert!(idle_resolved.load(Ordering::SeqCst));
        assert!(!agent.state().is_streaming);
    }

    /// README "message_end barrier": the beforeToolCall hook runs only after
    /// the assistant message_end listeners settled and the state includes
    /// the assistant message.
    #[tokio::test]
    async fn message_end_is_a_barrier_before_tool_preflight() {
        let (models, faux, model) = faux_models();
        faux.set_responses(vec![
            tool_call_response(
                vec![("call-1", "echo", serde_json::json!({}))],
                StopReason::ToolUse,
            ),
            text_response("done"),
        ]);
        let agent = Arc::new(Agent::new(
            AgentOptions {
                initial_state: AgentInitialState {
                    tools: vec![Arc::new(create_tool("echo"))],
                    model: Some(model.clone()),
                    ..AgentInitialState::default()
                },
                ..AgentOptions::default()
            },
            models,
        ));

        let message_end_settled = Arc::new(AtomicBool::new(false));
        {
            let message_end_settled = message_end_settled.clone();
            agent.subscribe(move |event: AgentEvent, _signal: CancellationToken| {
                let message_end_settled = message_end_settled.clone();
                Box::pin(async move {
                    if let AgentEvent::MessageEnd {
                        message: AgentMessage::Assistant(_),
                    } = event
                    {
                        // Give a racing tool preflight every chance to pass
                        // the barrier: it must not, the listener settles
                        // first.
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        message_end_settled.store(true, Ordering::SeqCst);
                    }
                })
            });
        }

        let saw_barrier = Arc::new(AtomicBool::new(false));
        let saw_assistant_in_state = Arc::new(AtomicBool::new(false));
        let hook: Arc<BeforeToolCallHook> = {
            let (saw_barrier, saw_assistant_in_state, agent) = (
                saw_barrier.clone(),
                saw_assistant_in_state.clone(),
                Arc::downgrade(&agent),
            );
            Arc::new(move |_context: BeforeToolCallContext| {
                let (saw_barrier, saw_assistant_in_state, agent) = (
                    saw_barrier.clone(),
                    saw_assistant_in_state.clone(),
                    agent.clone(),
                );
                Box::pin(async move {
                    saw_barrier.store(true, Ordering::SeqCst);
                    if let Some(agent) = agent.upgrade() {
                        let state = agent.state();
                        saw_assistant_in_state.store(
                            state
                                .messages
                                .last()
                                .map(|message: &AgentMessage| message.role())
                                == Some("assistant"),
                            Ordering::SeqCst,
                        );
                    }
                    BeforeToolCallOutcome::default()
                }) as BoxFuture<'static, BeforeToolCallOutcome>
            })
        };
        agent.runtime().before_tool_call = Some(hook);

        agent.prompt("run tool").await.unwrap();

        assert!(message_end_settled.load(Ordering::SeqCst));
        assert!(saw_barrier.load(Ordering::SeqCst));
        assert!(saw_assistant_in_state.load(Ordering::SeqCst));
    }

    // ---- abort (agent.test.ts:444-480, 678-683) ----

    /// Oracle "should pass the active abort signal to subscribers".
    #[tokio::test]
    async fn passes_the_active_abort_signal_to_subscribers() {
        // A throttled faux stream: one token per chunk at 50 tokens/second,
        // so a 4000-character response streams for about 80 seconds unless
        // aborted.
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
        let models = Arc::new(models);
        faux.set_responses(vec![FauxResponseStep::Message(Box::new(
            faux_assistant_message("x".repeat(4000), FauxMessageOptions::default()),
        ))]);

        let agent = Arc::new(Agent::new(faux_options(&model), models));
        let received = Arc::new(Mutex::new(None::<CancellationToken>));
        let first_event = Arc::new(Notify::new());
        {
            let (received, first_event) = (received.clone(), first_event.clone());
            agent.subscribe(move |_event: AgentEvent, signal: CancellationToken| {
                let (received, first_event) = (received.clone(), first_event.clone());
                Box::pin(async move {
                    if received.lock().unwrap().is_none() {
                        *received.lock().unwrap() = Some(signal);
                        first_event.notify_one();
                    }
                })
            });
        }

        let prompt_agent = agent.clone();
        let prompt_task = tokio::spawn(async move { prompt_agent.prompt("hello").await });
        first_event.notified().await;

        let signal = received.lock().unwrap().clone().expect("signal received");
        assert!(!signal.is_cancelled());

        agent.abort();
        prompt_task.await.unwrap().unwrap();

        assert!(signal.is_cancelled());
    }

    /// Oracle "should handle abort controller": abort with no active run is
    /// a no-op.
    #[test]
    fn abort_with_no_active_run_is_a_no_op() {
        let (models, _faux, _model) = faux_models();
        let agent = Agent::new(AgentOptions::default(), models);
        agent.abort();
        assert!(agent.signal().is_none());
    }

    // ---- late tool updates (agent.test.ts:482-621) ----

    /// Oracle "should ignore tool updates after the tool execution settles".
    #[tokio::test]
    async fn ignores_tool_updates_after_the_tool_execution_settles() {
        let (models, faux, model) = faux_models();
        let stashed = Arc::new(Mutex::new(None::<Arc<AgentToolUpdateCallback>>));
        let tool = {
            let stashed = stashed.clone();
            AgentTool {
                name: "delayed_tool".into(),
                label: "Delayed Tool".into(),
                description: "Captures progress callbacks".into(),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
                execute: Arc::new(
                    move |_id: String,
                          _params: serde_json::Value,
                          _signal: Option<CancellationToken>,
                          on_update: Option<Arc<AgentToolUpdateCallback>>| {
                        let stashed = stashed.clone();
                        Box::pin(async move {
                            if let Some(on_update) = &on_update {
                                *stashed.lock().unwrap() = Some(Arc::clone(on_update));
                                on_update(&AgentToolResult {
                                    content: vec![text_block("running")],
                                    details: Some(serde_json::json!({"status": "running"})),
                                    ..AgentToolResult::default()
                                });
                            }
                            Ok(AgentToolResult {
                                content: vec![text_block("ok")],
                                details: Some(serde_json::json!({"status": "done"})),
                                terminate: Some(true),
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
        };
        faux.set_responses(vec![tool_call_response(
            vec![("call-1", "delayed_tool", serde_json::json!({}))],
            StopReason::ToolUse,
        )]);

        let agent = Arc::new(Agent::new(
            AgentOptions {
                initial_state: AgentInitialState {
                    tools: vec![Arc::new(tool)],
                    model: Some(model.clone()),
                    ..AgentInitialState::default()
                },
                ..AgentOptions::default()
            },
            models,
        ));
        let events = Arc::new(Mutex::new(Vec::<AgentEvent>::new()));
        agent.subscribe(recording_listener(events.clone()));

        agent.prompt("run tool").await.unwrap();
        let event_count_after_prompt = events.lock().unwrap().len();

        // The stashed callback fires after the run settled: ignored.
        if let Some(late) = stashed.lock().unwrap().as_ref() {
            late(&AgentToolResult {
                content: vec![text_block("late")],
                details: Some(serde_json::json!({"status": "late"})),
                ..AgentToolResult::default()
            });
        }
        tokio::task::yield_now().await;

        let events = events.lock().unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, AgentEvent::ToolExecutionUpdate { .. }))
                .count(),
            1
        );
        assert_eq!(events.len(), event_count_after_prompt);
    }

    /// Oracle "should ignore a settled parallel tool update while another
    /// tool is still running".
    #[tokio::test]
    async fn ignores_a_settled_parallel_tool_update_while_another_tool_is_still_running() {
        let (models, faux, model) = faux_models();
        let settled_update = Arc::new(Mutex::new(None::<Arc<AgentToolUpdateCallback>>));
        let slow_started = Arc::new(Notify::new());
        let settled_ended = Arc::new(Notify::new());
        let release_slow = Arc::new(Notify::new());

        let settled_tool = {
            let settled_update = settled_update.clone();
            AgentTool {
                name: "settled_tool".into(),
                label: "Settled Tool".into(),
                description: "Captures progress callbacks".into(),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
                execute: Arc::new(
                    move |_id: String,
                          _params: serde_json::Value,
                          _signal: Option<CancellationToken>,
                          on_update: Option<Arc<AgentToolUpdateCallback>>| {
                        let settled_update = settled_update.clone();
                        Box::pin(async move {
                            if let Some(on_update) = on_update {
                                *settled_update.lock().unwrap() = Some(on_update);
                            }
                            Ok(AgentToolResult {
                                content: vec![text_block("done")],
                                details: Some(serde_json::json!({"status": "done"})),
                                terminate: Some(true),
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
        };
        let slow_tool = {
            let (slow_started, release_slow) = (slow_started.clone(), release_slow.clone());
            AgentTool {
                name: "slow_tool".into(),
                label: "Slow Tool".into(),
                description: "Keeps the agent run active".into(),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
                execute: Arc::new(
                    move |_id: String,
                          _params: serde_json::Value,
                          _signal: Option<CancellationToken>,
                          _on_update: Option<Arc<AgentToolUpdateCallback>>| {
                        let (slow_started, release_slow) =
                            (slow_started.clone(), release_slow.clone());
                        Box::pin(async move {
                            slow_started.notify_one();
                            release_slow.notified().await;
                            Ok(AgentToolResult {
                                content: vec![text_block("done")],
                                details: Some(serde_json::json!({"status": "done"})),
                                terminate: Some(true),
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
        };
        faux.set_responses(vec![tool_call_response(
            vec![
                ("call-1", "settled_tool", serde_json::json!({})),
                ("call-2", "slow_tool", serde_json::json!({})),
            ],
            StopReason::ToolUse,
        )]);

        let agent = Arc::new(Agent::new(
            AgentOptions {
                initial_state: AgentInitialState {
                    tools: vec![Arc::new(settled_tool), Arc::new(slow_tool)],
                    model: Some(model.clone()),
                    ..AgentInitialState::default()
                },
                ..AgentOptions::default()
            },
            models,
        ));
        let events = Arc::new(Mutex::new(Vec::<AgentEvent>::new()));
        {
            let events = events.clone();
            let settled_ended = settled_ended.clone();
            agent.subscribe(move |event: AgentEvent, _signal: CancellationToken| {
                let (events, settled_ended) = (events.clone(), settled_ended.clone());
                Box::pin(async move {
                    if let AgentEvent::ToolExecutionEnd { tool_call_id, .. } = &event {
                        if tool_call_id == "call-1" {
                            settled_ended.notify_one();
                        }
                    }
                    events.lock().unwrap().push(event);
                })
            });
        }

        let prompt_agent = agent.clone();
        let prompt_task = tokio::spawn(async move { prompt_agent.prompt("run tools").await });
        tokio::join!(slow_started.notified(), settled_ended.notified());
        let event_count_before_late_update = events.lock().unwrap().len();

        // call-1 settled; its stashed callback must not emit while the batch
        // is still running.
        if let Some(late) = settled_update.lock().unwrap().as_ref() {
            late(&AgentToolResult {
                content: vec![text_block("late")],
                details: Some(serde_json::json!({"status": "late"})),
                ..AgentToolResult::default()
            });
        }
        tokio::task::yield_now().await;
        assert_eq!(events.lock().unwrap().len(), event_count_before_late_update);

        release_slow.notify_one();
        prompt_task.await.unwrap().unwrap();

        let events = events.lock().unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, AgentEvent::ToolExecutionUpdate { .. }))
                .count(),
            0
        );
    }

    // ---- state mutators (agent.test.ts:623-656) ----

    /// Oracle "should update state with mutators".
    #[test]
    fn state_mutators_update_state() {
        let (models, _faux, model) = faux_models();
        let agent = Agent::new(AgentOptions::default(), models);

        agent.state().model = model.clone();
        assert_eq!(agent.state().model, model);

        agent.state().thinking_level = ThinkingLevel::High;
        assert_eq!(agent.state().thinking_level, ThinkingLevel::High);

        let tools = vec![Arc::new(create_tool("test"))];
        agent.state().tools = tools.clone();
        assert_eq!(agent.state().tools, tools);

        let messages = vec![user_message("Hello", TS)];
        agent.state().messages = messages.clone();
        assert_eq!(agent.state().messages, messages);

        let new_message = assistant_message("Hi");
        agent.state().messages.push(new_message.clone());
        assert_eq!(agent.state().messages.len(), 2);
        assert_eq!(agent.state().messages[1], new_message);

        agent.state().messages.clear();
        assert!(agent.state().messages.is_empty());
    }

    // ---- queues (agent.test.ts:658-676 + README accessors) ----

    /// Oracle "should support steering message queue".
    #[test]
    fn steer_queues_a_message_without_touching_the_transcript() {
        let (models, _faux, _model) = faux_models();
        let agent = Agent::new(AgentOptions::default(), models);
        let message = user_message("Steering message", TS);
        agent.steer(message.clone());
        assert!(!agent.state().messages.contains(&message));
    }

    /// Oracle "should support follow-up message queue".
    #[test]
    fn follow_up_queues_a_message_without_touching_the_transcript() {
        let (models, _faux, _model) = faux_models();
        let agent = Agent::new(AgentOptions::default(), models);
        let message = user_message("Follow-up message", TS);
        agent.follow_up(message.clone());
        assert!(!agent.state().messages.contains(&message));
    }

    /// The queue mode accessors and clear* surface (agent.ts:279-326).
    #[test]
    fn queue_accessors_cover_modes_and_clearing() {
        let (models, _faux, _model) = faux_models();
        let agent = Agent::new(AgentOptions::default(), models);
        assert_eq!(agent.steering_mode(), QueueMode::OneAtATime);
        assert_eq!(agent.follow_up_mode(), QueueMode::OneAtATime);
        assert!(!agent.has_queued_messages());

        agent.steer(user_message("s1", 1));
        agent.steer(user_message("s2", 2));
        agent.follow_up(user_message("f1", 3));
        assert!(agent.has_queued_messages());

        agent.set_steering_mode(QueueMode::All);
        assert_eq!(agent.steering_mode(), QueueMode::All);

        agent.clear_steering_queue();
        assert!(agent.has_queued_messages());

        agent.clear_all_queues();
        assert!(!agent.has_queued_messages());
    }

    // ---- in-run guards (agent.test.ts:685-793) ----

    /// Oracle "should reject reset while processing without corrupting the
    /// transcript".
    #[tokio::test]
    async fn rejects_reset_while_processing_without_corrupting_the_transcript() {
        let (models, faux, model) = faux_models();
        let stream_started = Arc::new(Notify::new());
        let release_response = Arc::new(Notify::new());
        faux.set_responses(vec![blocking_factory_step(
            stream_started.clone(),
            release_response.clone(),
            "Done",
        )]);

        let agent = Arc::new(Agent::new(faux_options(&model), models));
        let prompt_agent = agent.clone();
        let prompt_task = tokio::spawn(async move { prompt_agent.prompt("Hello").await });
        stream_started.notified().await;

        assert!(agent.state().is_streaming);
        assert_eq!(role_names(&agent.state()), ["user"]);
        let error = agent.reset().unwrap_err();
        assert!(error
            .to_string()
            .contains("Agent is already processing. Wait for completion before resetting."));
        assert!(agent.state().is_streaming);
        assert_eq!(role_names(&agent.state()), ["user"]);

        release_response.notify_one();
        prompt_task.await.unwrap().unwrap();

        assert!(!agent.state().is_streaming);
        assert_eq!(role_names(&agent.state()), ["user", "assistant"]);
    }

    /// Oracle "should throw when prompt() called while streaming".
    #[tokio::test]
    async fn prompt_while_streaming_rejects() {
        let (models, faux, model) = faux_models();
        let stream_started = Arc::new(Notify::new());
        let release_response = Arc::new(Notify::new());
        faux.set_responses(vec![blocking_factory_step(
            stream_started.clone(),
            release_response.clone(),
            "First response",
        )]);

        let agent = Arc::new(Agent::new(faux_options(&model), models));
        let prompt_agent = agent.clone();
        let first_prompt = tokio::spawn(async move { prompt_agent.prompt("First message").await });
        stream_started.notified().await;
        assert!(agent.state().is_streaming);

        let error = agent.prompt("Second message").await.unwrap_err();
        assert!(error.to_string().contains(
            "Agent is already processing a prompt. Use steer() or followUp() to queue \
             messages, or wait for completion."
        ));

        release_response.notify_one();
        first_prompt.await.unwrap().unwrap();
    }

    /// Oracle "should throw when continue() called while streaming".
    #[tokio::test]
    async fn continue_while_streaming_rejects() {
        let (models, faux, model) = faux_models();
        let stream_started = Arc::new(Notify::new());
        let release_response = Arc::new(Notify::new());
        faux.set_responses(vec![blocking_factory_step(
            stream_started.clone(),
            release_response.clone(),
            "First response",
        )]);

        let agent = Arc::new(Agent::new(
            AgentOptions {
                initial_state: AgentInitialState {
                    model: Some(model.clone()),
                    ..AgentInitialState::default()
                },
                ..AgentOptions::default()
            },
            models,
        ));
        let prompt_agent = agent.clone();
        let first_prompt = tokio::spawn(async move { prompt_agent.prompt("First message").await });
        stream_started.notified().await;
        assert!(agent.state().is_streaming);

        let error = agent.continue_run().await.unwrap_err();
        assert!(error
            .to_string()
            .contains("Agent is already processing. Wait for completion before continuing."));

        release_response.notify_one();
        first_prompt.await.unwrap().unwrap();
    }

    // ---- continue from an assistant tail (agent.test.ts:795-875) ----

    /// Oracle "continue() should process queued follow-up messages after an
    /// assistant turn".
    #[tokio::test]
    async fn continue_processes_queued_follow_up_messages_after_an_assistant_turn() {
        let (models, faux, model) = faux_models();
        faux.set_responses(vec![text_response("Processed")]);
        let agent = Arc::new(Agent::new(faux_options(&model), models));

        agent.state().messages = vec![
            user_message("Initial", TS - 10),
            assistant_message("Initial response"),
        ];
        agent.follow_up(user_message("Queued follow-up", TS));

        agent.continue_run().await.unwrap();

        let state = agent.state();
        assert!(state.messages.iter().any(|message| matches!(
            message,
            AgentMessage::User(user) if content_text(&user.content) == "Queued follow-up"
        )));
        assert_eq!(
            state.messages.last().map(|message| message.role()),
            Some("assistant")
        );
    }

    /// Oracle "continue() should keep one-at-a-time steering semantics from
    /// assistant tail": each drained steering message takes its own turn.
    #[tokio::test]
    async fn continue_keeps_one_at_a_time_steering_semantics_from_an_assistant_tail() {
        let (models, faux, model) = faux_models();
        faux.set_responses(vec![
            text_response("Processed 1"),
            text_response("Processed 2"),
        ]);
        let agent = Arc::new(Agent::new(faux_options(&model), models));

        agent.state().messages = vec![
            user_message("Initial", TS - 10),
            assistant_message("Initial response"),
        ];
        agent.steer(user_message("Steering 1", TS));
        agent.steer(user_message("Steering 2", TS + 1));

        agent.continue_run().await.unwrap();

        let state = agent.state();
        let roles: Vec<&str> = state
            .messages
            .iter()
            .map(|message| message.role())
            .rev()
            .take(4)
            .rev()
            .collect();
        assert_eq!(roles, ["user", "assistant", "user", "assistant"]);
        assert_eq!(
            faux.state().lock().unwrap().call_count,
            2,
            "two drained steering messages, two turns"
        );
    }

    /// Oracle "continue() from a user tail runs the low-level continuation".
    #[tokio::test]
    async fn continue_from_a_user_tail_runs_the_continuation() {
        let (models, faux, model) = faux_models();
        faux.set_responses(vec![text_response("Continued")]);
        let agent = Arc::new(Agent::new(faux_options(&model), models));

        agent.state().messages = vec![user_message("Initial", TS - 10)];
        agent.continue_run().await.unwrap();

        let state = agent.state();
        assert_eq!(role_names(&state), ["user", "assistant"]);
        let last = state.messages.last().cloned().unwrap();
        let AgentMessage::Assistant(assistant) = last else {
            panic!("expected assistant message");
        };
        let text = assistant
            .content
            .iter()
            .filter_map(|block| match block {
                crate::ai::types::message::AssistantBlock::Text(text) => Some(text.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(text, "Continued");
    }

    /// Oracle "No messages to continue from" for an empty or all-system
    /// transcript, and "Cannot continue from message role: assistant" when
    /// no queue offers a message.
    #[tokio::test]
    async fn continue_validates_the_transcript_before_running() {
        let (models, _faux, _model) = faux_models();
        let agent = Arc::new(Agent::new(AgentOptions::default(), models));

        let error = agent.continue_run().await.unwrap_err();
        assert!(error.to_string().contains("No messages to continue from"));

        agent.state().messages = vec![AgentMessage::System(SystemMessage {
            content: StringOrBlocks::Text("prompt".into()),
            sections: None,
            tools_added: None,
            tools_removed: None,
            timestamp: 0,
        })];
        let error = agent.continue_run().await.unwrap_err();
        assert!(error.to_string().contains("No messages to continue from"));

        agent.state().messages = vec![
            user_message("Initial", TS),
            assistant_message("Initial response"),
        ];
        let error = agent.continue_run().await.unwrap_err();
        assert!(error
            .to_string()
            .contains("Cannot continue from message role: assistant"));
    }

    // ---- forwarded hooks and options (agent.test.ts:877-986) ----

    /// Oracle "keeps legacy prepareNextTurn signal callback behavior"
    /// (adapted: the port's single hook form already receives the turn
    /// context; the forwarded AbortSignal does not exist in the port's hook
    /// signatures).
    #[tokio::test]
    async fn prepare_next_turn_receives_the_turn_context_between_requests() {
        let (models, faux, model) = faux_models();
        let executed = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let prepare_calls = Arc::new(AtomicUsize::new(0));
        let saw_roles = Arc::new(Mutex::new(Vec::<String>::new()));
        let mut options = AgentOptions {
            initial_state: AgentInitialState {
                tools: vec![Arc::new(echo_tool(executed.clone()))],
                model: Some(model.clone()),
                ..AgentInitialState::default()
            },
            ..AgentOptions::default()
        };
        {
            let (prepare_calls, saw_roles) = (prepare_calls.clone(), saw_roles.clone());
            options.prepare_next_turn = Some(Arc::new(move |context: PrepareNextTurnContext| {
                let (prepare_calls, saw_roles) = (prepare_calls.clone(), saw_roles.clone());
                Box::pin(async move {
                    prepare_calls.fetch_add(1, Ordering::SeqCst);
                    *saw_roles.lock().unwrap() = context
                        .context
                        .messages
                        .iter()
                        .map(|message| message.role().to_string())
                        .collect();
                    None
                })
            }));
        }
        let agent = Arc::new(Agent::new(options, models));
        faux.set_responses(vec![
            tool_call_response(
                vec![("tool-1", "noop", serde_json::json!({}))],
                StopReason::ToolUse,
            ),
            text_response("done"),
        ]);

        agent.prompt("start").await.unwrap();

        assert_eq!(faux.state().lock().unwrap().call_count, 2);
        assert_eq!(prepare_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            *saw_roles.lock().unwrap(),
            ["system", "user", "assistant", "toolResult"]
                .iter()
                .map(|role| role.to_string())
                .collect::<Vec<_>>()
        );
    }

    /// Oracle "forwards shouldStopAfterTurn through AgentOptions".
    #[tokio::test]
    async fn forwards_should_stop_after_turn_through_options() {
        let (models, faux, model) = faux_models();
        let executed = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let saw_roles = Arc::new(Mutex::new(Vec::<String>::new()));
        let roles_writer = saw_roles.clone();
        let mut options = AgentOptions {
            initial_state: AgentInitialState {
                tools: vec![Arc::new(echo_tool(executed.clone()))],
                model: Some(model.clone()),
                ..AgentInitialState::default()
            },
            ..AgentOptions::default()
        };
        options.should_stop_after_turn =
            Some(Arc::new(move |context: ShouldStopAfterTurnContext| {
                let roles_writer = roles_writer.clone();
                Box::pin(async move {
                    *roles_writer.lock().unwrap() = context
                        .context
                        .messages
                        .iter()
                        .map(|message| message.role().to_string())
                        .collect();
                    true
                })
            }));
        let agent = Arc::new(Agent::new(options, models));
        faux.set_responses(vec![tool_call_response(
            vec![("tool-1", "noop", serde_json::json!({}))],
            StopReason::ToolUse,
        )]);

        agent.prompt("start").await.unwrap();

        assert_eq!(faux.state().lock().unwrap().call_count, 1);
        assert_eq!(*executed.lock().unwrap(), [serde_json::json!({})]);
        assert_eq!(
            *saw_roles.lock().unwrap(),
            ["system", "user", "assistant", "toolResult"]
                .iter()
                .map(|role| role.to_string())
                .collect::<Vec<_>>()
        );
    }

    /// Oracle "forwards sessionId to streamFunction options" — the port's
    /// stream function is the Models collection, so the faux provider
    /// factory observes the session id on the request options.
    #[tokio::test]
    async fn forwards_session_id_to_the_stream_function() {
        let (models, faux, model) = faux_models();
        let seen = Arc::new(Mutex::new(Vec::<Option<String>>::new()));
        let recorder: FauxResponseFactory = {
            let seen = seen.clone();
            Arc::new(move |args: FauxFactoryArgs| {
                let seen = seen.clone();
                Box::pin(async move {
                    seen.lock().unwrap().push(
                        args.options
                            .as_ref()
                            .and_then(|o| o.stream.session_id.clone()),
                    );
                    Ok(faux_assistant_message("ok", FauxMessageOptions::default()))
                }) as BoxFuture<'static, Result<AssistantMessage, String>>
            })
        };
        faux.set_responses(vec![
            FauxResponseStep::Factory(recorder.clone()),
            FauxResponseStep::Factory(recorder),
        ]);

        let agent = Agent::new(
            AgentOptions {
                session_id: Some("session-abc".into()),
                initial_state: AgentInitialState {
                    model: Some(model.clone()),
                    ..AgentInitialState::default()
                },
                ..AgentOptions::default()
            },
            models,
        );

        agent.prompt("hello").await.unwrap();

        agent.runtime().session_id = Some("session-def".into());
        assert_eq!(agent.runtime().session_id.as_deref(), Some("session-def"));

        agent.prompt("hello again").await.unwrap();

        assert_eq!(
            *seen.lock().unwrap(),
            vec![Some("session-abc".into()), Some("session-def".into())]
        );
    }

    // ---- prompt input normalization (agent.ts:406-423) ----

    /// Upstream `prompt(text, images)`: the user message carries the text
    /// block followed by the images.
    #[tokio::test]
    async fn prompt_with_images_builds_text_and_image_blocks() {
        let (models, faux, model) = faux_models();
        faux.set_responses(vec![text_response("ok")]);

        let agent = Agent::new(faux_options(&model), models);
        agent
            .prompt(PromptInput::with_images(
                "look",
                vec![ImageContent {
                    data: "aGk=".into(),
                    mime_type: "image/png".into(),
                }],
            ))
            .await
            .unwrap();

        let state = agent.state();
        let Some(AgentMessage::User(user)) = state.messages.first() else {
            panic!("expected user message");
        };
        let crate::ai::types::message::StringOrBlocks::Blocks(blocks) = &user.content else {
            panic!("expected content blocks");
        };
        assert_eq!(blocks.len(), 2);
        assert!(matches!(
            blocks[0],
            crate::ai::types::message::TextOrImageBlock::Text(_)
        ));
        assert!(matches!(
            blocks[1],
            crate::ai::types::message::TextOrImageBlock::Image(_)
        ));
    }

    /// A no-argument echo-style tool recording its executed arguments
    /// (upstream noop/echo tools of the forwarded-hook oracles).
    fn echo_tool(executed: Arc<Mutex<Vec<serde_json::Value>>>) -> AgentTool {
        AgentTool {
            name: "noop".into(),
            label: "Noop".into(),
            description: "Noop tool".into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            execute: Arc::new(
                move |_id: String,
                      params: serde_json::Value,
                      _signal: Option<CancellationToken>,
                      _on_update: Option<Arc<AgentToolUpdateCallback>>| {
                    let executed = executed.clone();
                    Box::pin(async move {
                        executed.lock().unwrap().push(params);
                        Ok(AgentToolResult {
                            content: vec![text_block("tool complete")],
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
}

#[cfg(test)]
#[path = "agent/event_wire_tests.rs"]
mod event_wire_tests;
