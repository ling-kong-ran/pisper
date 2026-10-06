//! The faux provider, ported from upstream `packages/ai/src/providers/faux.ts`:
//! a fully scripted [`Provider`] for tests — responses are queued as
//! [`AssistantMessage`]s (or response factories) and replayed FIFO through the
//! normal [`ApiImpl`] stream contract, with the upstream delta choreography
//! (thinking/text/toolCall start/delta/end), token-estimated usage with
//! session-scoped prompt-cache accounting, optional `tokensPerSecond` pacing,
//! call counting, multi-model catalogs, and the deferred-response machinery.
//!
//! Relationship to `src/agent/faux.rs` (deliberate name overlap, disclosed in
//! the task): the agent-level `FauxProvider` is a test-only [`ApiImpl`] that
//! replays hand-built event vectors verbatim — it predates this module and
//! serves the agent tests. This module is the real upstream faux provider:
//! it scripts *messages*, generates the event choreography itself, computes
//! usage, and builds a full [`Provider`] for the [`Models`](super::Models)
//! collection. The two serve different layers and do not share code.
//!
//! Port deviations (each mirrors an existing M2b ruling):
//! - `options.signal` is the port's non-serialized
//!   [`CancellationToken`](tokio_util::sync::CancellationToken) (upstream
//!   `AbortSignal`): a pre-aborted stream settles with the
//!   `createAbortedMessage` error event before `start`, and a cancellation
//!   between paced chunks (or before a block starts) settles the same way
//!   (upstream `streamWithDeltas`'s `signal?.aborted` checks). Stream
//!   `onResponse` now reports the synthetic 200 response; injectable `fetch`
//!   and callbacks on the separate deferred handle remain unported.
//! - `fetchDeferred`/`cancelDeferred` live on the [`FauxCore`] handle (and
//!   the [`FauxProviderHandle`]), not on the [`ApiImpl`] trait body's
//!   required methods — the M2b trait kept the stream surface only.
//!   [`FauxApi::stream_deferred`] bridges the routing-side
//!   `Models::stream_deferred` dispatch onto [`FauxCore::fetch_deferred`]
//!   (reporting the upstream synthetic 200 through
//!   `options.callbacks.on_response`); `FauxApi::cancel_deferred` bridges
//!   `Models::cancel_deferred` onto [`FauxCore::cancel_deferred`] the same
//!   way (the capability probe answers `true`, faux.ts:695). The
//!   bookkeeping (`pendingFetches`, cancellation flags, final-message
//!   memoization) is ported faithfully; upstream's `fetchOptions.signal`
//!   has no port input there yet, so deferred fetches run unabortable.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;

use crate::ai::api::ApiImpl;
use crate::ai::auth::types::{ApiKeyAuth, ApiKeyAuthInput, AuthError, AuthResult, ProviderAuth};
use crate::ai::transcript::{get_system_message_text, TranscriptContext};
use crate::ai::types::content::{TextContent, ThinkingContent, ToolCall};
use crate::ai::types::events::{AssistantMessageEvent, ErrorReason, SuccessReason};
use crate::ai::types::message::{
    AssistantBlock, AssistantMessage, Message, StringOrBlocks, TextOrImageBlock,
};
use crate::ai::types::options::{DeferredFlag, DeferredHandle, SimpleStreamOptions};
use crate::ai::types::primitives::CacheRetention;
use crate::ai::types::primitives::{ModelCost, StopReason, Usage, UsageCost};
use crate::ai::types::{Model, ModelInput};
use crate::ai::{now_ms, ProviderConfig};
use tokio_util::sync::CancellationToken;

use super::provider::{create_provider, ApiImpls, CreateProviderOptions};
use super::Provider;

const DEFAULT_API: &str = "faux";
const DEFAULT_PROVIDER: &str = "faux";
const DEFAULT_MODEL_ID: &str = "faux-1";
const DEFAULT_MODEL_NAME: &str = "Faux Model";
const DEFAULT_BASE_URL: &str = "http://localhost:0";
const DEFAULT_MIN_TOKEN_SIZE: usize = 3;
const DEFAULT_MAX_TOKEN_SIZE: usize = 5;

fn default_usage() -> Usage {
    Usage {
        input: 0,
        output: 0,
        cache_read: 0,
        cache_write: 0,
        cache_write_1h: None,
        reasoning: None,
        total_tokens: 0,
        cost: UsageCost::default(),
    }
}

/// Upstream `FauxModelDefinition` (faux.ts:41-49).
#[derive(Debug, Clone, Default)]
pub struct FauxModelDefinition {
    pub id: String,
    /// Display name; defaults to the id (upstream `name?`).
    pub name: Option<String>,
    /// Defaults to false (upstream `reasoning?`).
    pub reasoning: Option<bool>,
    /// Defaults to `["text", "image"]` (upstream `input?`).
    pub input: Option<Vec<ModelInput>>,
    /// Provider input limits metadata (upstream `inputLimits?`, the faux
    /// delta from the unified-catalog slice).
    pub input_limits: Option<crate::ai::types::ModelInputLimits>,
    /// Defaults to zero rates (upstream `cost?`).
    pub cost: Option<ModelCost>,
    /// Defaults to 128000 (upstream `contextWindow?`).
    pub context_window: Option<u64>,
    /// Defaults to 16384 (upstream `maxTokens?`).
    pub max_tokens: Option<u64>,
}

/// Upstream `string | FauxContentBlock | FauxContentBlock[]` (faux.ts:78):
/// the content argument of [`faux_assistant_message`]. `FauxContentBlock`
/// is upstream's `TextContent | ThinkingContent | ToolCall` — the same
/// tagged union as the message layer's [`AssistantBlock`].
#[derive(Debug, Clone)]
pub enum FauxContent {
    Text(String),
    Block(AssistantBlock),
    Blocks(Vec<AssistantBlock>),
}

impl From<&str> for FauxContent {
    fn from(text: &str) -> Self {
        FauxContent::Text(text.to_string())
    }
}

impl From<String> for FauxContent {
    fn from(text: String) -> Self {
        FauxContent::Text(text)
    }
}

impl From<AssistantBlock> for FauxContent {
    fn from(block: AssistantBlock) -> Self {
        FauxContent::Block(block)
    }
}

impl From<Vec<AssistantBlock>> for FauxContent {
    fn from(blocks: Vec<AssistantBlock>) -> Self {
        FauxContent::Blocks(blocks)
    }
}

/// Upstream `fauxText` (faux.ts:53-55).
pub fn faux_text(text: impl Into<String>) -> AssistantBlock {
    AssistantBlock::Text(TextContent {
        text: text.into(),
        text_signature: None,
    })
}

/// Upstream `fauxThinking` (faux.ts:57-59).
pub fn faux_thinking(thinking: impl Into<String>) -> AssistantBlock {
    AssistantBlock::Thinking(ThinkingContent {
        thinking: thinking.into(),
        thinking_signature: None,
        redacted: None,
    })
}

/// Upstream `fauxToolCall` options (faux.ts:61): `{ id?: string }`.
#[derive(Debug, Clone, Default)]
pub struct FauxToolCallOptions {
    /// Explicit tool-call id; defaults to a random one.
    pub id: Option<String>,
}

/// Upstream `fauxToolCall` (faux.ts:61-68): a tool call with an explicit or
/// random id.
pub fn faux_tool_call(
    name: impl Into<String>,
    arguments: serde_json::Value,
    options: FauxToolCallOptions,
) -> AssistantBlock {
    AssistantBlock::ToolCall(ToolCall {
        id: options.id.unwrap_or_else(|| random_id("tool")),
        name: name.into(),
        arguments,
        thought_signature: None,
        namespace: None,
    })
}

/// Upstream `fauxAssistantMessage` options (faux.ts:79-85).
#[derive(Debug, Clone, Default)]
pub struct FauxMessageOptions {
    /// Defaults to [`StopReason::Stop`] (upstream `"stop"`).
    pub stop_reason: Option<StopReason>,
    pub deferred: Option<DeferredHandle>,
    pub error_message: Option<String>,
    pub response_id: Option<String>,
    /// Defaults to the current time (upstream `Date.now()`).
    pub timestamp: Option<i64>,
}

/// Upstream `fauxAssistantMessage` (faux.ts:77-100): a scripted assistant
/// message stamped with the default faux api/provider/model and zero usage
/// (usage is re-estimated per request by [`with_usage_estimate`]).
pub fn faux_assistant_message(
    content: impl Into<FauxContent>,
    options: FauxMessageOptions,
) -> AssistantMessage {
    let content = match content.into() {
        FauxContent::Text(text) => vec![faux_text(text)],
        FauxContent::Block(block) => vec![block],
        FauxContent::Blocks(blocks) => blocks,
    };
    AssistantMessage {
        content,
        api: DEFAULT_API.to_string(),
        provider: DEFAULT_PROVIDER.to_string(),
        model: DEFAULT_MODEL_ID.to_string(),
        response_model: None,
        response_id: options.response_id,
        provider_thinking_level: None,
        diagnostics: None,
        usage: default_usage(),
        stop_reason: options.stop_reason.unwrap_or(StopReason::Stop),
        deferred: options.deferred,
        error_message: options.error_message,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: options.timestamp.unwrap_or_else(now_ms),
    }
}

/// Upstream `FauxProviderState` (faux.ts:102-106).
#[derive(Debug, Default, Clone)]
pub struct FauxProviderState {
    pub call_count: u64,
    pub deferred_fetch_count: u64,
    pub cancelled_deferred: Vec<DeferredHandle>,
}

/// Shared handle to the provider state — handed to response factories
/// (upstream passes the same object by reference).
pub type FauxStateHandle = Arc<Mutex<FauxProviderState>>;

/// Upstream `FauxResponseFactory` (faux.ts:108-113): build a response at
/// request time from the transcript, options, state, and request model.
/// The upstream `(context, options, state, model) => AssistantMessage |
/// Promise<AssistantMessage>` signature maps to owned arguments plus a
/// boxed future; an upstream throw (sync or rejected promise) is the
/// `Err` channel carrying the thrown message.
pub type FauxResponseFactory = Arc<
    dyn Fn(FauxFactoryArgs) -> BoxFuture<'static, Result<AssistantMessage, String>> + Send + Sync,
>;

/// The owned arguments of a [`FauxResponseFactory`] invocation.
pub struct FauxFactoryArgs {
    pub context: TranscriptContext,
    pub options: Option<SimpleStreamOptions>,
    pub state: FauxStateHandle,
    pub model: Model,
}

/// Upstream `FauxResponseStep` (faux.ts:115).
#[derive(Clone)]
pub enum FauxResponseStep {
    Message(Box<AssistantMessage>),
    Factory(FauxResponseFactory),
}

impl From<AssistantMessage> for FauxResponseStep {
    fn from(message: AssistantMessage) -> Self {
        FauxResponseStep::Message(Box::new(message))
    }
}

/// Upstream `RegisterFauxProviderOptions.deferred` (faux.ts:121-125).
#[derive(Debug, Clone, Default)]
pub struct FauxDeferredOptions {
    /// Number of fetches that return the original handle before the
    /// scripted response becomes ready.
    pub pending_fetches: Option<u32>,
    pub poll_after_ms: Option<u64>,
}

/// Upstream `RegisterFauxProviderOptions.tokenSize` (faux.ts:127-130).
#[derive(Debug, Clone, Copy, Default)]
pub struct FauxTokenSize {
    pub min: Option<usize>,
    pub max: Option<usize>,
}

/// Upstream `RegisterFauxProviderOptions` (faux.ts:117-131).
#[derive(Debug, Clone, Default)]
pub struct FauxProviderOptions {
    /// Api id; defaults to a unique per-provider id (upstream
    /// `randomId(DEFAULT_API)`), so multiple faux providers never collide
    /// in per-api dispatch maps.
    pub api: Option<String>,
    /// Provider id; defaults to `"faux"`.
    pub provider: Option<String>,
    /// Model catalog; defaults to one `faux-1` model.
    pub models: Vec<FauxModelDefinition>,
    pub deferred: Option<FauxDeferredOptions>,
    /// Streaming pacing in tokens/second; `None`/`<= 0` streams instantly.
    pub tokens_per_second: Option<f64>,
    pub token_size: Option<FauxTokenSize>,
}

/// One deferred response awaiting fetch (faux.ts:448-460). The entry sits
/// behind its own mutex so a fetch can memoize the final message and flip
/// the cancellation flag while other fetches hold the map lock.
struct DeferredEntry {
    handle: DeferredHandle,
    step: FauxResponseStep,
    context: TranscriptContext,
    options: Option<SimpleStreamOptions>,
    model: Model,
    pending_fetches: u32,
    cancelled: bool,
    final_message: Option<AssistantMessage>,
}

/// Shared core state ([`FauxCore`]), Arc-cloned into the spawned stream
/// tasks and the [`FauxApi`] implementation.
struct FauxInner {
    api: String,
    provider: String,
    min_token_size: usize,
    max_token_size: usize,
    tokens_per_second: Option<f64>,
    poll_after_ms: Option<u64>,
    default_pending_fetches: u32,
    models: Vec<Model>,
    state: FauxStateHandle,
    pending: Mutex<VecDeque<FauxResponseStep>>,
    prompt_cache: Mutex<HashMap<String, String>>,
    deferred: Mutex<HashMap<String, Mutex<DeferredEntry>>>,
}

/// Upstream `createFauxCore` (faux.ts:436-673): the cloned handle over the
/// faux provider's shared state.
#[derive(Clone)]
pub struct FauxCore(Arc<FauxInner>);

impl FauxCore {
    /// Upstream `createFauxCore(options)`.
    pub fn new(options: FauxProviderOptions) -> Self {
        let api = options.api.unwrap_or_else(|| random_id(DEFAULT_API));
        let provider = options
            .provider
            .unwrap_or_else(|| DEFAULT_PROVIDER.to_string());
        let raw_min = options
            .token_size
            .and_then(|size| size.min)
            .unwrap_or(DEFAULT_MIN_TOKEN_SIZE);
        let raw_max = options
            .token_size
            .and_then(|size| size.max)
            .unwrap_or(DEFAULT_MAX_TOKEN_SIZE);
        let min_token_size = raw_min.clamp(1, raw_max);
        let max_token_size = raw_max.max(min_token_size);
        let deferred = options.deferred.unwrap_or_default();

        let definitions = if options.models.is_empty() {
            vec![FauxModelDefinition {
                id: DEFAULT_MODEL_ID.to_string(),
                name: Some(DEFAULT_MODEL_NAME.to_string()),
                reasoning: Some(false),
                input: Some(vec![ModelInput::Text, ModelInput::Image]),
                input_limits: None,
                cost: Some(ModelCost::default()),
                context_window: Some(128_000),
                max_tokens: Some(16_384),
            }]
        } else {
            options.models
        };
        let models = definitions
            .into_iter()
            .map(|definition| Model {
                name: definition.name.unwrap_or_else(|| definition.id.clone()),
                id: definition.id,
                api: api.clone(),
                provider: provider.clone(),
                base_url: DEFAULT_BASE_URL.to_string(),
                r#type: None,
                reasoning: definition.reasoning.unwrap_or(false),
                thinking_level_map: None,
                prompt_cache: None,
                input: definition
                    .input
                    .unwrap_or_else(|| vec![ModelInput::Text, ModelInput::Image]),
                input_limits: definition.input_limits,
                cost: definition.cost.unwrap_or_default(),
                context_window: definition.context_window.unwrap_or(128_000),
                max_tokens: definition.max_tokens.unwrap_or(16_384),
                sampling_params: None,
                sampling_params_by_thinking_level: None,
                headers: None,
                compat: None,
            })
            .collect();

        FauxCore(Arc::new(FauxInner {
            api,
            provider,
            min_token_size,
            max_token_size,
            tokens_per_second: options.tokens_per_second,
            poll_after_ms: deferred.poll_after_ms,
            default_pending_fetches: deferred.pending_fetches.unwrap_or(0),
            models,
            state: Arc::new(Mutex::new(FauxProviderState::default())),
            pending: Mutex::new(VecDeque::new()),
            prompt_cache: Mutex::new(HashMap::new()),
            deferred: Mutex::new(HashMap::new()),
        }))
    }

    /// The faux api id (upstream `api`).
    pub fn api(&self) -> &str {
        &self.0.api
    }

    /// The faux provider id (upstream `provider`).
    pub fn provider(&self) -> &str {
        &self.0.provider
    }

    /// The faux model catalog (upstream `models`).
    pub fn models(&self) -> &[Model] {
        &self.0.models
    }

    /// Upstream `getModel()`/`getModel(id)` (faux.ts:644-651): `None` for the
    /// id returns the first model; with an id, the matching one.
    pub fn get_model(&self, id: Option<&str>) -> Option<Model> {
        match id {
            None => self.0.models.first().cloned(),
            Some(id) => self.0.models.iter().find(|model| model.id == id).cloned(),
        }
    }

    /// The shared provider state (upstream `state`).
    pub fn state(&self) -> FauxStateHandle {
        Arc::clone(&self.0.state)
    }

    /// Upstream `setResponses` (faux.ts:663-665): replace the queue.
    pub fn set_responses(&self, responses: Vec<FauxResponseStep>) {
        *lock(&self.0.pending) = responses.into_iter().collect();
    }

    /// Upstream `appendResponses` (faux.ts:666-668).
    pub fn append_responses(&self, responses: Vec<FauxResponseStep>) {
        self.0
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend(responses);
    }

    /// Upstream `getPendingResponseCount` (faux.ts:669-671).
    pub fn get_pending_response_count(&self) -> usize {
        lock(&self.0.pending).len()
    }

    /// Upstream `stream`/`streamSimple` (faux.ts:503-565): pop the next step,
    /// count the call, and run the response choreography on a spawned task.
    /// The endpoint config is ignored — the scripted message is the response.
    pub(crate) fn stream_internal(
        &self,
        model: &Model,
        context: &TranscriptContext,
        options: Option<SimpleStreamOptions>,
    ) -> tokio::sync::mpsc::Receiver<AssistantMessageEvent> {
        let step = lock(&self.0.pending).pop_front();
        {
            let mut state = lock(&self.0.state);
            state.call_count += 1;
        }
        let (tx, rx) = tokio::sync::mpsc::channel(EVENT_CHANNEL_CAPACITY);
        let core = self.clone();
        let model = model.clone();
        let context = context.clone();
        tokio::spawn(async move {
            let signal = options
                .as_ref()
                .and_then(|options| options.stream.signal.clone());
            // Upstream faux.ts:510: synthetic HTTP metadata precedes even
            // the exhausted-response error. Faux has no onPayload boundary.
            if let Some(options) = &options {
                if let Err(error) = options
                    .stream
                    .callbacks
                    .response(
                        crate::ai::types::request_callbacks::ProviderResponse {
                            status: 200,
                            headers: Default::default(),
                        },
                        &model,
                    )
                    .await
                {
                    report_error(&tx, error.to_string(), &core, &model).await;
                    return;
                }
            }
            let Some(step) = step else {
                // Upstream faux.ts:511-521: exhausted queue settles the
                // stream with an error message (usage-estimated).
                let message = with_usage_estimate(
                    create_error_message(
                        "No more faux responses queued",
                        &core.0.api,
                        &core.0.provider,
                        &model.id,
                    ),
                    &context,
                    options.as_ref(),
                    &core.0.prompt_cache,
                );
                let _ = tx
                    .send(AssistantMessageEvent::Error {
                        reason: ErrorReason::Error,
                        error: message,
                    })
                    .await;
                return;
            };

            // Upstream `if (options?.deferred)` (faux.ts:524): a TRUTHY
            // `deferred` option submits for deferred resolution. `false`
            // (and absent) is an ordinary request — the former existence
            // check treated `Some(false)` as deferred, diverging from the
            // JS truthiness semantics (structural-slice report).
            let deferred_requested = options
                .as_ref()
                .and_then(|options| options.deferred.as_ref())
                .is_some_and(|flag| match flag {
                    DeferredFlag::Bool(value) => *value,
                    DeferredFlag::Object { .. } => true,
                });
            if deferred_requested {
                // Deferred request (faux.ts:524-550): register the entry and
                // stream the handle-carrying deferred message.
                let handle = DeferredHandle {
                    provider: model.provider.clone(),
                    model_id: model.id.clone(),
                    api: model.api.clone(),
                    id: random_id("deferred"),
                    expires_at: None,
                    poll_after_ms: core.0.poll_after_ms,
                    data: None,
                };
                let pending_fetches = core.0.default_pending_fetches;
                let entry = DeferredEntry {
                    handle: handle.clone(),
                    step,
                    context: context.clone(),
                    options: options.clone(),
                    model: model.clone(),
                    pending_fetches,
                    cancelled: false,
                    final_message: None,
                };
                lock(&core.0.deferred).insert(handle.id.clone(), Mutex::new(entry));
                let result = stream_with_deltas(
                    &tx,
                    create_deferred_message(&model, &handle),
                    core.0.min_token_size,
                    core.0.max_token_size,
                    core.0.tokens_per_second,
                    signal.as_ref(),
                )
                .await;
                report_stream_failure(&tx, result, &core, &model).await;
                return;
            }

            // Upstream faux.ts:552-558: resolution throws settle the stream
            // with the thrown message as the error result.
            match resolve_response(&core, step, &context, options.as_ref(), &model).await {
                Ok(message) => {
                    let result = stream_with_deltas(
                        &tx,
                        message,
                        core.0.min_token_size,
                        core.0.max_token_size,
                        core.0.tokens_per_second,
                        signal.as_ref(),
                    )
                    .await;
                    report_stream_failure(&tx, result, &core, &model).await;
                }
                Err(error) => report_error(&tx, error, &core, &model).await,
            }
        });
        rx
    }

    /// Upstream `fetchDeferred` (faux.ts:567-631). Lives on the handle (the
    /// M2b [`ApiImpl`] dropped the deferred surface); see the module docs.
    pub fn fetch_deferred(
        &self,
        model: &Model,
        handle: &DeferredHandle,
    ) -> tokio::sync::mpsc::Receiver<AssistantMessageEvent> {
        {
            let mut state = lock(&self.0.state);
            state.deferred_fetch_count += 1;
        }
        let (tx, rx) = tokio::sync::mpsc::channel(EVENT_CHANNEL_CAPACITY);
        let core = self.clone();
        let model = model.clone();
        let handle = handle.clone();
        tokio::spawn(async move {
            // Phase 1 (synchronous -- the std-lock guards drop before any
            // await): validate the handle and claim what this fetch
            // produces (faux.ts:578-614).
            enum Outcome {
                Pending(DeferredHandle),
                Final(Box<AssistantMessage>),
                Resolve(
                    Box<FauxResponseStep>,
                    TranscriptContext,
                    Box<Option<SimpleStreamOptions>>,
                    Box<Model>,
                ),
                Fail(String),
            }
            let outcome = {
                let map = lock(&core.0.deferred);
                map.get(&handle.id)
                    .map(|entry_cell| {
                        let mut entry = lock(entry_cell);
                        if entry.handle.provider != handle.provider
                            || entry.handle.model_id != handle.model_id
                            || entry.handle.api != handle.api
                        {
                            Outcome::Fail(format!("Unknown faux deferred response: {}", handle.id))
                        } else if entry.cancelled {
                            Outcome::Fail(format!(
                                "Faux deferred response was cancelled: {}",
                                handle.id
                            ))
                        } else if entry.pending_fetches > 0 {
                            entry.pending_fetches -= 1;
                            Outcome::Pending(entry.handle.clone())
                        } else {
                            match entry.final_message.clone() {
                                Some(message) => Outcome::Final(Box::new(message)),
                                // Claim the resolution: run outside the
                                // lock, then memoize (faux.ts:602-614).
                                None => Outcome::Resolve(
                                    Box::new(entry.step.clone()),
                                    entry.context.clone(),
                                    Box::new(entry.options.clone()),
                                    Box::new(entry.model.clone()),
                                ),
                            }
                        }
                    })
                    .unwrap_or_else(|| {
                        Outcome::Fail(format!("Unknown faux deferred response: {}", handle.id))
                    })
            };

            let outcome = match outcome {
                Outcome::Fail(message) => {
                    report_error(&tx, message, &core, &model).await;
                    return;
                }
                Outcome::Pending(handle) => PendingOrFinal::Pending(handle),
                Outcome::Final(message) => PendingOrFinal::Final(message),
                Outcome::Resolve(step, context, options, model) => {
                    // Submission options drop the deferred flag
                    // (faux.ts:602-608); a thrown factory memoizes the error
                    // message as the final response (faux.ts:609-614).
                    let mut submission = *options;
                    if let Some(options) = submission.as_mut() {
                        options.deferred = None;
                    }
                    let final_message =
                        match resolve_response(&core, *step, &context, submission.as_ref(), &model)
                            .await
                        {
                            Ok(message) => message,
                            Err(error) => {
                                create_error_message(error, core.api(), core.provider(), &model.id)
                            }
                        };
                    if let Some(entry_cell) = lock(&core.0.deferred).get(&handle.id) {
                        let mut entry = lock(entry_cell);
                        if entry.final_message.is_none() {
                            entry.final_message = Some(final_message.clone());
                        }
                    }
                    PendingOrFinal::Final(Box::new(final_message))
                }
            };

            match outcome {
                PendingOrFinal::Pending(pending_handle) => {
                    let result = stream_with_deltas(
                        &tx,
                        create_deferred_message(&model, &pending_handle),
                        core.0.min_token_size,
                        core.0.max_token_size,
                        core.0.tokens_per_second,
                        // Upstream `fetchOptions?.signal`; the deferred fetch
                        // surface has no options input yet (module docs).
                        None,
                    )
                    .await;
                    report_stream_failure(&tx, result, &core, &model).await;
                }
                PendingOrFinal::Final(message) => {
                    let result = stream_with_deltas(
                        &tx,
                        *message,
                        core.0.min_token_size,
                        core.0.max_token_size,
                        core.0.tokens_per_second,
                        None,
                    )
                    .await;
                    report_stream_failure(&tx, result, &core, &model).await;
                }
            }
        });
        rx
    }

    /// Upstream `cancelDeferred` (faux.ts:633-642): record the cancelled
    /// handle and flag the entry, so later fetches fail.
    pub async fn cancel_deferred(&self, handle: &DeferredHandle) {
        lock(&self.0.state).cancelled_deferred.push(handle.clone());
        if let Some(entry) = lock(&self.0.deferred).get(&handle.id) {
            lock(entry).cancelled = true;
        }
    }
}

/// What a [`FauxCore::fetch_deferred`] resolution produced.
enum PendingOrFinal {
    Pending(DeferredHandle),
    Final(Box<AssistantMessage>),
}

/// Upstream `resolveResponse` (faux.ts:488-501): run a plain or factory
/// step, re-stamp the message with this request's identity
/// (`cloneMessage`), and re-estimate usage against the transcript. An
/// `Err` is a thrown factory (faux.ts:494): callers convert it to the
/// terminal error message their upstream catch blocks build.
async fn resolve_response(
    core: &FauxCore,
    step: FauxResponseStep,
    context: &TranscriptContext,
    options: Option<&SimpleStreamOptions>,
    model: &Model,
) -> Result<AssistantMessage, String> {
    let mut resolved = match step {
        FauxResponseStep::Message(message) => *message,
        FauxResponseStep::Factory(factory) => {
            factory(FauxFactoryArgs {
                context: context.clone(),
                options: options.cloned(),
                state: core.state(),
                model: model.clone(),
            })
            .await?
        }
    };
    // cloneMessage (faux.ts:281-291): re-stamp with this request's identity,
    // then re-estimate usage against the transcript.
    resolved.api = core.api().to_string();
    resolved.provider = core.provider().to_string();
    resolved.model = model.id.clone();
    Ok(with_usage_estimate(
        resolved,
        context,
        options,
        &core.0.prompt_cache,
    ))
}

/// Send the stream failure event for a choreography result: the only
/// failure is the "no stop reason" contract violation (faux.ts:423-425),
/// which upstream converts to an error message in the catch block.
async fn report_stream_failure(
    tx: &tokio::sync::mpsc::Sender<AssistantMessageEvent>,
    result: Result<(), AssistantMessage>,
    core: &FauxCore,
    model: &Model,
) {
    if result.is_err() {
        report_error(tx, "Faux response ended without a stop reason", core, model).await;
    }
}

/// Build and push the terminal error event (faux.ts:555-558).
async fn report_error(
    tx: &tokio::sync::mpsc::Sender<AssistantMessageEvent>,
    message: impl std::fmt::Display,
    core: &FauxCore,
    model: &Model,
) {
    let error = create_error_message(message, core.api(), core.provider(), &model.id);
    let _ = tx
        .send(AssistantMessageEvent::Error {
            reason: ErrorReason::Error,
            error,
        })
        .await;
}

/// Upstream `streamWithDeltas` (faux.ts:338-434): emit the delta
/// choreography for one scripted message. `Err` carries the message whose
/// `stopReason` was `"pending"` (faux.ts:423-425).
// The Err payload is the scripted AssistantMessage itself; every consumer
// only tests `is_err()` (report_stream_failure), so boxing would add
// indirection for no functional gain (same trade as AgentMessage's
// large_enum_variant allow).
#[allow(clippy::result_large_err)]
async fn stream_with_deltas(
    tx: &tokio::sync::mpsc::Sender<AssistantMessageEvent>,
    message: AssistantMessage,
    min_token_size: usize,
    max_token_size: usize,
    tokens_per_second: Option<f64>,
    signal: Option<&CancellationToken>,
) -> Result<(), AssistantMessage> {
    // Upstream `createAbortedMessage` (faux.ts:321-328): the partial keeps
    // its state and settles aborted with the canonical message.
    async fn abort_stream(
        tx: &tokio::sync::mpsc::Sender<AssistantMessageEvent>,
        partial: &AssistantMessage,
    ) {
        let mut aborted = partial.clone();
        aborted.stop_reason = StopReason::Aborted;
        aborted.error_message = Some("Request was aborted".to_string());
        aborted.timestamp = now_ms();
        let _ = tx
            .send(AssistantMessageEvent::Error {
                reason: ErrorReason::Aborted,
                error: aborted,
            })
            .await;
    }

    // The start event carries the initial message structure: the scripted
    // metadata with empty content and a pending stop reason
    // (faux.ts:346, 354). A pre-aborted signal settles before `start`
    // (faux.ts:347-353).
    let mut partial = message.clone();
    partial.content = Vec::new();
    partial.stop_reason = StopReason::Pending;
    if signal.is_some_and(CancellationToken::is_cancelled) {
        abort_stream(tx, &partial).await;
        return Ok(());
    }
    if tx
        .send(AssistantMessageEvent::Start {
            message: partial.clone(),
        })
        .await
        .is_err()
    {
        return Ok(());
    }

    for (index, block) in message.content.iter().enumerate() {
        // faux.ts:358-365: the per-block abort check.
        if signal.is_some_and(CancellationToken::is_cancelled) {
            abort_stream(tx, &partial).await;
            return Ok(());
        }
        match block {
            AssistantBlock::Thinking(thinking) => {
                if tx
                    .send(AssistantMessageEvent::ThinkingStart {
                        content_index: index,
                    })
                    .await
                    .is_err()
                {
                    return Ok(());
                }
                for chunk in
                    split_string_by_token_size(&thinking.thinking, min_token_size, max_token_size)
                {
                    schedule_chunk(&chunk, tokens_per_second).await;
                    // faux.ts:370-378: the post-schedule abort check.
                    if signal.is_some_and(CancellationToken::is_cancelled) {
                        abort_stream(tx, &partial).await;
                        return Ok(());
                    }
                    if tx
                        .send(AssistantMessageEvent::ThinkingDelta {
                            content_index: index,
                            delta: chunk,
                        })
                        .await
                        .is_err()
                    {
                        return Ok(());
                    }
                }
                if tx
                    .send(AssistantMessageEvent::ThinkingEnd {
                        content_index: index,
                        content: thinking.thinking.clone(),
                    })
                    .await
                    .is_err()
                {
                    return Ok(());
                }
            }
            AssistantBlock::Text(text) => {
                if tx
                    .send(AssistantMessageEvent::TextStart {
                        content_index: index,
                    })
                    .await
                    .is_err()
                {
                    return Ok(());
                }
                for chunk in split_string_by_token_size(&text.text, min_token_size, max_token_size)
                {
                    schedule_chunk(&chunk, tokens_per_second).await;
                    // faux.ts:392-400: the post-schedule abort check.
                    if signal.is_some_and(CancellationToken::is_cancelled) {
                        abort_stream(tx, &partial).await;
                        return Ok(());
                    }
                    if tx
                        .send(AssistantMessageEvent::TextDelta {
                            content_index: index,
                            delta: chunk,
                        })
                        .await
                        .is_err()
                    {
                        return Ok(());
                    }
                }
                if tx
                    .send(AssistantMessageEvent::TextEnd {
                        content_index: index,
                        content: text.text.clone(),
                    })
                    .await
                    .is_err()
                {
                    return Ok(());
                }
            }
            AssistantBlock::ToolCall(tool_call) => {
                if tx
                    .send(AssistantMessageEvent::ToolcallStart {
                        content_index: index,
                    })
                    .await
                    .is_err()
                {
                    return Ok(());
                }
                let arguments = serde_json::to_string(&tool_call.arguments).unwrap_or_default();
                for chunk in split_string_by_token_size(&arguments, min_token_size, max_token_size)
                {
                    schedule_chunk(&chunk, tokens_per_second).await;
                    // faux.ts:408-416: the post-schedule abort check.
                    if signal.is_some_and(CancellationToken::is_cancelled) {
                        abort_stream(tx, &partial).await;
                        return Ok(());
                    }
                    if tx
                        .send(AssistantMessageEvent::ToolcallDelta {
                            content_index: index,
                            delta: chunk,
                        })
                        .await
                        .is_err()
                    {
                        return Ok(());
                    }
                }
                if tx
                    .send(AssistantMessageEvent::ToolcallEnd {
                        content_index: index,
                        tool_call: tool_call.clone(),
                    })
                    .await
                    .is_err()
                {
                    return Ok(());
                }
            }
        }
    }

    match message.stop_reason {
        // faux.ts:423-425.
        StopReason::Pending => Err(message),
        // faux.ts:426-430.
        StopReason::Error | StopReason::Aborted => {
            let reason = if message.stop_reason == StopReason::Aborted {
                ErrorReason::Aborted
            } else {
                ErrorReason::Error
            };
            let _ = tx
                .send(AssistantMessageEvent::Error {
                    reason,
                    error: message,
                })
                .await;
            Ok(())
        }
        // faux.ts:432-433.
        stop_reason => {
            let reason = match stop_reason {
                StopReason::Length => SuccessReason::Length,
                StopReason::ToolUse => SuccessReason::ToolUse,
                StopReason::Deferred => SuccessReason::Deferred,
                _ => SuccessReason::Stop,
            };
            let _ = tx
                .send(AssistantMessageEvent::Done { reason, message })
                .await;
            Ok(())
        }
    }
}

/// Upstream `scheduleChunk` (faux.ts:330-336): pace one chunk at the
/// configured tokens/second; unset or non-positive rates stream instantly.
async fn schedule_chunk(chunk: &str, tokens_per_second: Option<f64>) {
    if let Some(tokens_per_second) = tokens_per_second.filter(|rate| *rate > 0.0) {
        let delay_ms = (estimate_tokens(chunk) as f64 / tokens_per_second * 1000.0).max(0.0);
        tokio::time::sleep(Duration::from_millis(delay_ms as u64)).await;
    }
}

/// Upstream `createDeferredMessage` (faux.ts:293-305).
fn create_deferred_message(model: &Model, handle: &DeferredHandle) -> AssistantMessage {
    AssistantMessage {
        content: Vec::new(),
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        diagnostics: None,
        usage: default_usage(),
        stop_reason: StopReason::Deferred,
        deferred: Some(handle.clone()),
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: now_ms(),
    }
}

/// Upstream `createErrorMessage` (faux.ts:307-319).
fn create_error_message(
    message: impl std::fmt::Display,
    api: &str,
    provider: &str,
    model_id: &str,
) -> AssistantMessage {
    AssistantMessage {
        content: Vec::new(),
        api: api.to_string(),
        provider: provider.to_string(),
        model: model_id.to_string(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        diagnostics: None,
        usage: default_usage(),
        stop_reason: StopReason::Error,
        deferred: None,
        error_message: Some(message.to_string()),
        raw_stop_reason: None,
        end_turn: None,
        timestamp: now_ms(),
    }
}

/// Upstream `estimateTokens` (faux.ts:157-159): a quarter of the text
/// length, rounded up. JS `.length` counts UTF-16 units; the port counts
/// scalar characters — identical for the BMP fixtures these tests use.
fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

/// Upstream `randomId` (faux.ts:161-163): `{prefix}:{now}:{base36}`. The
/// upstream suffix is `Math.random().toString(36).slice(2)`; the port
/// renders a random u64 in base36 (same shape, denser digits).
fn random_id(prefix: &str) -> String {
    let mut bytes = [0u8; 8];
    rand::fill(&mut bytes);
    format!(
        "{prefix}:{}:{}",
        now_ms(),
        to_base36(u64::from_le_bytes(bytes))
    )
}

/// JS `Number.prototype.toString(36)`: lowercase `0-9a-z` digits.
fn to_base36(mut value: u64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".to_string();
    }
    let mut digits = Vec::new();
    while value > 0 {
        digits.push(DIGITS[(value % 36) as usize]);
        value /= 36;
    }
    digits.reverse();
    String::from_utf8(digits).expect("base36 digits are ASCII")
}

/// Upstream `contentToText` (faux.ts:165-177).
fn content_to_text(content: &StringOrBlocks) -> String {
    match content {
        StringOrBlocks::Text(text) => text.clone(),
        StringOrBlocks::Blocks(blocks) => blocks
            .iter()
            .map(|block| match block {
                TextOrImageBlock::Text(text) => text.text.clone(),
                TextOrImageBlock::Image(image) => {
                    format!("[image:{}:{}]", image.mime_type, image.data.chars().count())
                }
            })
            .collect::<Vec<String>>()
            .join("\n"),
    }
}

/// Upstream `assistantContentToText` (faux.ts:179-191).
fn assistant_content_to_text(content: &[AssistantBlock]) -> String {
    content
        .iter()
        .map(|block| match block {
            AssistantBlock::Text(text) => text.text.clone(),
            AssistantBlock::Thinking(thinking) => thinking.thinking.clone(),
            AssistantBlock::ToolCall(tool_call) => format!(
                "{}:{}",
                tool_call.name,
                serde_json::to_string(&tool_call.arguments).unwrap_or_default()
            ),
        })
        .collect::<Vec<String>>()
        .join("\n")
}

/// Upstream `toolResultToText` (faux.ts:193-195).
fn tool_result_to_text(message: &crate::ai::types::message::ToolResultMessage) -> String {
    let mut parts = vec![message.tool_name.clone()];
    parts.extend(message.content.iter().map(|block| match block {
        TextOrImageBlock::Text(text) => text.text.clone(),
        TextOrImageBlock::Image(image) => {
            format!("[image:{}:{}]", image.mime_type, image.data.chars().count())
        }
    }));
    parts.join("\n")
}

/// Upstream `messageToText` (faux.ts:197-214).
fn message_to_text(message: &Message) -> String {
    match message {
        Message::System(system) => {
            let mut parts = vec![get_system_message_text(system)];
            for tool in system.tools_removed.iter().flatten() {
                parts.push(format!(
                    "tool-:{}",
                    serde_json::to_string(tool).unwrap_or_default()
                ));
            }
            for tool in system.tools_added.iter().flatten() {
                parts.push(format!(
                    "tool+:{}",
                    serde_json::to_string(tool).unwrap_or_default()
                ));
            }
            parts
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<String>>()
                .join("\n")
        }
        Message::User(user) => content_to_text(&user.content),
        Message::Assistant(assistant) => assistant_content_to_text(&assistant.content),
        Message::ToolResult(tool_result) => tool_result_to_text(tool_result),
    }
}

/// Upstream `serializeContext` (faux.ts:216-218).
fn serialize_context(context: &TranscriptContext) -> String {
    context
        .messages()
        .iter()
        .map(|message| format!("{}:{}", message_role(message), message_to_text(message)))
        .collect::<Vec<String>>()
        .join("\n\n")
}

/// The upstream `message.role` wire literal.
fn message_role(message: &Message) -> &'static str {
    match message {
        Message::System(_) => "system",
        Message::User(_) => "user",
        Message::Assistant(_) => "assistant",
        Message::ToolResult(_) => "toolResult",
    }
}

/// Upstream `commonPrefixLength` (faux.ts:220-227), character-based.
fn common_prefix_length(a: &str, b: &str) -> usize {
    a.chars()
        .zip(b.chars())
        .take_while(|(left, right)| left == right)
        .count()
}

/// Upstream `withUsageEstimate` (faux.ts:229-267): token-estimate the
/// prompt and output, and — for session-tagged requests that allow caching —
/// split the prompt across cache read/write against the session's previous
/// prompt.
fn with_usage_estimate(
    mut message: AssistantMessage,
    context: &TranscriptContext,
    options: Option<&SimpleStreamOptions>,
    prompt_cache: &Mutex<HashMap<String, String>>,
) -> AssistantMessage {
    let prompt_text = serialize_context(context);
    let prompt_tokens = estimate_tokens(&prompt_text);
    let output_tokens = estimate_tokens(&assistant_content_to_text(&message.content));
    let mut input = prompt_tokens as u64;
    let mut cache_read = 0u64;
    let mut cache_write = 0u64;

    let session_id = options
        .and_then(|options| options.stream.session_id.as_deref())
        .filter(|session| !session.is_empty());
    // Upstream `options?.cacheRetention !== "none"`: absent options still
    // allow caching.
    let caching_allowed = options
        .map(|options| options.stream.cache_retention != Some(CacheRetention::None))
        .unwrap_or(true);

    if let (Some(session_id), true) = (session_id, caching_allowed) {
        let mut cache = lock(prompt_cache);
        match cache.get(session_id) {
            Some(previous_prompt) => {
                let cached_chars = common_prefix_length(previous_prompt, &prompt_text);
                // Upstream `previousPrompt.slice(0, cachedChars)` and
                // `promptText.slice(cachedChars)`: slice by char count, so
                // translate the count to a byte offset for UTF-8 slicing.
                let prefix_byte = previous_prompt
                    .char_indices()
                    .nth(cached_chars)
                    .map(|(offset, _)| offset)
                    .unwrap_or(previous_prompt.len());
                cache_read = estimate_tokens(&previous_prompt[..prefix_byte]) as u64;
                let suffix: String = prompt_text.chars().skip(cached_chars).collect();
                cache_write = estimate_tokens(&suffix) as u64;
                input = (prompt_tokens as u64).saturating_sub(cache_read);
            }
            None => {
                cache_write = prompt_tokens as u64;
            }
        }
        cache.insert(session_id.to_string(), prompt_text);
    }

    message.usage = Usage {
        input,
        output: output_tokens as u64,
        cache_read,
        cache_write,
        cache_write_1h: None,
        reasoning: None,
        total_tokens: input + output_tokens as u64 + cache_read + cache_write,
        cost: UsageCost::default(),
    };
    message
}

/// Upstream `splitStringByTokenSize` (faux.ts:269-279): chunks of
/// `max(1, tokenSize * 4)` characters with `tokenSize` random in
/// `[min, max]`; empty input still yields one empty chunk.
fn split_string_by_token_size(
    text: &str,
    min_token_size: usize,
    max_token_size: usize,
) -> Vec<String> {
    let mut chunks = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0usize;
    while index < chars.len() {
        let spread = max_token_size.saturating_sub(min_token_size) + 1;
        let token_size = min_token_size + random_below(spread as u64) as usize;
        let char_size = (token_size * 4).max(1);
        let end = (index + char_size).min(chars.len());
        chunks.push(chars[index..end].iter().collect());
        index = end;
    }
    if chunks.is_empty() {
        chunks.push(String::new());
    }
    chunks
}

/// A random value in `[0, bound)` from a filled u64 (the `Math.random()`
/// stand-in; modulo bias is irrelevant for chunk sizing).
fn random_below(bound: u64) -> u64 {
    if bound == 0 {
        return 0;
    }
    let mut bytes = [0u8; 8];
    rand::fill(&mut bytes);
    u64::from_le_bytes(bytes) % bound
}

fn lock<T>(guard: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    guard
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Event-channel capacity, matching the `Models` routing layer.
const EVENT_CHANNEL_CAPACITY: usize = 64;

/// The [`ApiImpl`] half of the faux provider (upstream attaches `core.stream`
/// /`core.streamSimple` to the provider's api object, faux.ts:691-696).
#[derive(Clone)]
struct FauxApi(FauxCore);

impl ApiImpl for FauxApi {
    fn supports_request_callbacks(&self) -> bool {
        true
    }

    fn stream(
        &self,
        _cfg: &ProviderConfig,
        model: &Model,
        ctx: &TranscriptContext,
        options: &crate::ai::types::options::StreamOptions,
    ) -> tokio::sync::mpsc::Receiver<AssistantMessageEvent> {
        self.0.stream_internal(
            model,
            ctx,
            Some(SimpleStreamOptions {
                stream: options.clone(),
                ..SimpleStreamOptions::default()
            }),
        )
    }

    /// Upstream `provider.fetchDeferred` (faux.ts:567-631) joined to the
    /// routing surface: `Models::stream_deferred` dispatches here and the
    /// call delegates to [`FauxCore::fetch_deferred`]. The upstream
    /// `fetchOptions.onResponse` synthetic 200 (faux.ts:581) is reported
    /// through `options.callbacks.on_response` before any event flows, so
    /// response metadata is captured first. `signal`/`onPayload` remain
    /// unported (see the module docs: deferred fetches run unabortable and
    /// faux builds no request payload).
    fn stream_deferred(
        &self,
        _cfg: &ProviderConfig,
        model: &Model,
        handle: &DeferredHandle,
        options: &crate::ai::types::options::StreamOptions,
    ) -> tokio::sync::mpsc::Receiver<AssistantMessageEvent> {
        let inner = self.0.fetch_deferred(model, handle);
        let (tx, rx) = tokio::sync::mpsc::channel(EVENT_CHANNEL_CAPACITY);
        let on_response = options.callbacks.on_response.clone();
        let model = model.clone();
        tokio::spawn(async move {
            if let Some(on_response) = on_response {
                let _ = on_response(
                    crate::ai::types::request_callbacks::ProviderResponse {
                        status: 200,
                        headers: Default::default(),
                    },
                    model,
                )
                .await;
            }
            let mut inner = inner;
            while let Some(event) = inner.recv().await {
                if tx.send(event).await.is_err() {
                    break;
                }
            }
        });
        rx
    }

    fn stream_simple(
        &self,
        _cfg: &ProviderConfig,
        model: &Model,
        ctx: &TranscriptContext,
        options: &SimpleStreamOptions,
    ) -> tokio::sync::mpsc::Receiver<AssistantMessageEvent> {
        self.0.stream_internal(model, ctx, Some(options.clone()))
    }

    /// Upstream `faux.ts:695` attaches `cancelDeferred: core.cancelDeferred`
    /// to the provider's api object: the faux provider carries the deferred
    /// cancellation capability, so the probe answers true and the routed
    /// dispatch lands on [`FauxCore::cancel_deferred`].
    fn supports_deferred_cancel(&self) -> bool {
        true
    }

    fn cancel_deferred(
        &self,
        _cfg: &ProviderConfig,
        _model: &Model,
        handle: &DeferredHandle,
        _options: &crate::ai::types::options::StreamOptions,
    ) -> futures::future::BoxFuture<'static, Result<(), String>> {
        let core = self.0.clone();
        let handle = handle.clone();
        Box::pin(async move {
            core.cancel_deferred(&handle).await;
            Ok(())
        })
    }
}

/// Upstream `FauxProviderRegistration`/`FauxProviderHandle` (faux.ts:133-155):
/// the provider plus the scripting surface. The upstream `api` field (the
/// api id string) reads through [`FauxCore::api`].
pub struct FauxProviderHandle {
    /// The faux provider, ready to register with a [`Models`](super::Models)
    /// collection (upstream `provider`).
    pub provider: Arc<dyn Provider>,
    core: FauxCore,
}

impl FauxProviderHandle {
    /// The faux api id (upstream `handle.api`).
    pub fn api(&self) -> &str {
        self.core.api()
    }

    /// The faux provider id (upstream `core.provider`).
    pub fn provider_id(&self) -> &str {
        self.core.provider()
    }

    /// The faux model catalog (upstream `handle.models`).
    pub fn models(&self) -> &[Model] {
        self.core.models()
    }

    /// Upstream `getModel()`/`getModel(id)`.
    pub fn get_model(&self, id: Option<&str>) -> Option<Model> {
        self.core.get_model(id)
    }

    /// The shared provider state (upstream `handle.state`).
    pub fn state(&self) -> FauxStateHandle {
        self.core.state()
    }

    /// Upstream `setResponses`.
    pub fn set_responses(&self, responses: Vec<FauxResponseStep>) {
        self.core.set_responses(responses);
    }

    /// Upstream `appendResponses`.
    pub fn append_responses(&self, responses: Vec<FauxResponseStep>) {
        self.core.append_responses(responses);
    }

    /// Upstream `getPendingResponseCount`.
    pub fn get_pending_response_count(&self) -> usize {
        self.core.get_pending_response_count()
    }

    /// Upstream `fetchDeferred` (handle-level; see the module docs).
    pub fn fetch_deferred(
        &self,
        model: &Model,
        handle: &DeferredHandle,
    ) -> tokio::sync::mpsc::Receiver<AssistantMessageEvent> {
        self.core.fetch_deferred(model, handle)
    }

    /// Upstream `cancelDeferred` (handle-level; see the module docs).
    pub async fn cancel_deferred(&self, handle: &DeferredHandle) {
        self.core.cancel_deferred(handle).await;
    }

    /// The underlying core handle: the raw `stream`/`streamSimple` and
    /// deferred entry points, which the [`ApiImpl`] routing surface dropped
    /// (see the module docs) but tests scripting deferred responses drive
    /// directly.
    pub fn core(&self) -> &FauxCore {
        &self.core
    }
}

/// Upstream `fauxProvider(options)` (faux.ts:685-708): the scripted
/// provider built on an explicit [`Models`](super::Models)-compatible
/// [`Provider`]:
///
/// ```ignore
/// let faux = faux_provider(FauxProviderOptions::default());
/// let mut models = create_models(CreateModelsOptions::default());
/// models.set_provider(faux.provider.clone());
/// faux.set_responses(vec![faux_assistant_message("hi", Default::default()).into()]);
/// ```
pub fn faux_provider(options: FauxProviderOptions) -> FauxProviderHandle {
    let core = FauxCore::new(options);
    struct FauxAuth;

    impl ApiKeyAuth for FauxAuth {
        fn name(&self) -> &str {
            "Faux"
        }

        fn resolve<'a>(
            &'a self,
            _input: ApiKeyAuthInput<'a>,
        ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
            Box::pin(async { Ok(Some(AuthResult::default())) })
        }
    }

    let provider = create_provider(CreateProviderOptions {
        id: core.provider().to_string(),
        name: None,
        base_url: None,
        headers: None,
        auth: ProviderAuth {
            api_key: Some(Arc::new(FauxAuth)),
            oauth: None,
        },
        models: core
            .models()
            .to_vec()
            .into_iter()
            .map(crate::ai::types::AnyModel::Chat)
            .collect(),
        fetch_models: None,
        filter_models: None,
        filter_all_models: None,
        api: ApiImpls::Single(Arc::new(FauxApi(core.clone()))),
        images: crate::ai::models::provider::ImagesImpls::new(),
        classifiers: crate::ai::models::provider::ClassifiersImpls::new(),
    });
    FauxProviderHandle { provider, core }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::transcript::normalize_context;
    use crate::ai::types::content::ImageContent;
    use crate::ai::types::events::AssistantMessageEvent;
    use crate::ai::types::message::{ToolResultMessage, UserMessage};
    use crate::ai::types::options::{DeferredFlag, StreamOptions};
    use crate::ai::types::tool::Tool;

    fn faux() -> FauxProviderHandle {
        faux_provider(FauxProviderOptions::default())
    }

    fn transcript(text: &str) -> TranscriptContext {
        normalize_context(&crate::ai::Context {
            system_prompt: None,
            messages: vec![Message::User(UserMessage {
                content: StringOrBlocks::Text(text.to_string()),
                timestamp: 1758240000000,
            })],
            tools: None,
        })
    }

    fn stream_simple(
        handle: &FauxProviderHandle,
        context: &TranscriptContext,
        options: Option<SimpleStreamOptions>,
    ) -> tokio::sync::mpsc::Receiver<AssistantMessageEvent> {
        let model = handle.get_model(None).unwrap();
        handle.core().stream_internal(&model, context, options)
    }

    async fn drain(
        mut rx: tokio::sync::mpsc::Receiver<AssistantMessageEvent>,
    ) -> Vec<AssistantMessageEvent> {
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    }

    fn event_names(events: &[AssistantMessageEvent]) -> Vec<&'static str> {
        events.iter().map(|event| event.event_type()).collect()
    }

    fn done_message(events: &[AssistantMessageEvent]) -> &AssistantMessage {
        match events.last().expect("stream produced events") {
            AssistantMessageEvent::Done { message, .. } => message,
            other => panic!("expected done event, got {other:?}"),
        }
    }

    /// A scripted text message streams start -> text_start -> delta ->
    /// text_end -> done with the default faux identity and token-estimated
    /// usage (upstream faux.ts:389-404, 432-433).
    #[tokio::test]
    async fn scripted_text_message_streams_the_delta_choreography() {
        let handle = faux();
        handle.set_responses(vec![faux_assistant_message(
            "hello",
            FauxMessageOptions::default(),
        )
        .into()]);
        let events = drain(stream_simple(&handle, &transcript("hi"), None)).await;

        assert_eq!(
            event_names(&events),
            ["start", "text_start", "text_delta", "text_end", "done"]
        );
        match &events[2] {
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta,
            } => {
                // 5 chars fit in one chunk at min/max sizes 3..=5 (chars*4).
                assert_eq!(delta, "hello");
            }
            other => panic!("expected text delta, got {other:?}"),
        }
        match &events[3] {
            AssistantMessageEvent::TextEnd {
                content_index: 0,
                content,
            } => {
                assert_eq!(content, "hello");
            }
            other => panic!("expected text end, got {other:?}"),
        }

        let message = done_message(&events);
        assert_eq!(message.api, handle.api());
        assert_eq!(message.provider, "faux");
        assert_eq!(message.model, "faux-1");
        assert_eq!(message.stop_reason, StopReason::Stop);
        // Prompt "user:hi" (7 chars -> 2 tokens); output "hello" (5 -> 2).
        assert_eq!(message.usage.input, 2);
        assert_eq!(message.usage.output, 2);
        assert_eq!(message.usage.cache_read, 0);
        // No session id: no cache write either.
        assert_eq!(message.usage.cache_write, 0);
        assert_eq!(message.usage.total_tokens, 4);
    }

    // ---- abort oracles (faux-provider.test.ts "aborting" block) ----

    fn paced_handle(tokens_per_second: f64) -> FauxProviderHandle {
        faux_provider(FauxProviderOptions {
            tokens_per_second: Some(tokens_per_second),
            token_size: Some(FauxTokenSize {
                min: Some(3),
                max: Some(3),
            }),
            ..FauxProviderOptions::default()
        })
    }

    fn options_with_signal(token: CancellationToken) -> Option<SimpleStreamOptions> {
        Some(SimpleStreamOptions {
            stream: StreamOptions {
                signal: Some(token),
                ..StreamOptions::default()
            },
            ..SimpleStreamOptions::default()
        })
    }

    /// Collects events, cancelling the token after the first delta of
    /// `delta_type` (matched by event type name).
    async fn collect_aborting_on(
        rx: tokio::sync::mpsc::Receiver<AssistantMessageEvent>,
        token: CancellationToken,
        delta_type: &str,
    ) -> Vec<AssistantMessageEvent> {
        let mut rx = rx;
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            let cancelled = event.event_type() == delta_type;
            events.push(event);
            if cancelled {
                token.cancel();
            }
        }
        events
    }

    /// Oracle "supports aborting before the first chunk": a pre-cancelled
    /// signal settles the stream with the single aborted error event before
    /// any choreography (faux.ts:347-353).
    #[tokio::test]
    async fn aborting_before_the_first_chunk_yields_one_error_event() {
        let handle = paced_handle(50.0);
        handle.set_responses(vec![faux_assistant_message(
            "abcdefghijklmnopqrstuvwxyz",
            FauxMessageOptions::default(),
        )
        .into()]);
        let token = CancellationToken::new();
        token.cancel();
        let options = SimpleStreamOptions {
            stream: StreamOptions {
                signal: Some(token),
                ..StreamOptions::default()
            },
            ..SimpleStreamOptions::default()
        };
        let events = drain(stream_simple(&handle, &transcript("hi"), Some(options))).await;

        assert_eq!(events.len(), 1);
        match events.first().expect("one event") {
            AssistantMessageEvent::Error { reason, error } => {
                assert_eq!(*reason, ErrorReason::Aborted);
                assert_eq!(error.stop_reason, StopReason::Aborted);
            }
            other => panic!("expected error event, got {other:?}"),
        }
    }

    /// Oracle "supports aborting mid-text stream when paced": cancellation
    /// between paced chunks stops the text block before its end event.
    #[tokio::test]
    async fn aborting_mid_text_stream_stops_before_the_text_end() {
        let handle = paced_handle(100.0);
        handle.set_responses(vec![faux_assistant_message(
            "abcdefghijklmnopqrstuvwxyz",
            FauxMessageOptions::default(),
        )
        .into()]);
        let token = CancellationToken::new();
        let events = collect_aborting_on(
            stream_simple(
                &handle,
                &transcript("hi"),
                options_with_signal(token.clone()),
            ),
            token,
            "text_delta",
        )
        .await;

        let names = event_names(&events);
        assert_eq!(
            names.iter().filter(|name| **name == "text_delta").count(),
            1
        );
        assert!(names.contains(&"text_start"));
        assert!(names.contains(&"text_delta"));
        assert!(names.contains(&"error"));
        assert!(!names.contains(&"text_end"));
    }

    /// Oracle "supports aborting mid-thinking stream when paced".
    #[tokio::test]
    async fn aborting_mid_thinking_stream_stops_before_the_thinking_end() {
        let handle = paced_handle(100.0);
        handle.set_responses(vec![faux_assistant_message(
            vec![faux_thinking("abcdefghijklmnopqrstuvwxyz")],
            FauxMessageOptions::default(),
        )
        .into()]);
        let token = CancellationToken::new();
        let events = collect_aborting_on(
            stream_simple(
                &handle,
                &transcript("hi"),
                options_with_signal(token.clone()),
            ),
            token,
            "thinking_delta",
        )
        .await;

        let names = event_names(&events);
        assert_eq!(
            names
                .iter()
                .filter(|name| **name == "thinking_delta")
                .count(),
            1
        );
        assert!(names.contains(&"thinking_start"));
        assert!(names.contains(&"thinking_delta"));
        assert!(names.contains(&"error"));
        assert!(!names.contains(&"thinking_end"));
    }

    /// Oracle "supports aborting mid-toolcall stream when paced".
    #[tokio::test]
    async fn aborting_mid_toolcall_stream_stops_before_the_toolcall_end() {
        let handle = paced_handle(100.0);
        handle.set_responses(vec![faux_assistant_message(
            vec![faux_tool_call(
                "echo",
                serde_json::json!({"text": "abcdefghijklmnopqrstuvwxyz", "count": 123456789}),
                FauxToolCallOptions {
                    id: Some("tool-1".to_string()),
                },
            )],
            FauxMessageOptions {
                stop_reason: Some(StopReason::ToolUse),
                ..FauxMessageOptions::default()
            },
        )
        .into()]);
        let token = CancellationToken::new();
        let events = collect_aborting_on(
            stream_simple(
                &handle,
                &transcript("hi"),
                options_with_signal(token.clone()),
            ),
            token,
            "toolcall_delta",
        )
        .await;

        let names = event_names(&events);
        assert_eq!(
            names
                .iter()
                .filter(|name| **name == "toolcall_delta")
                .count(),
            1
        );
        assert!(names.contains(&"toolcall_start"));
        assert!(names.contains(&"toolcall_delta"));
        assert!(names.contains(&"error"));
        assert!(!names.contains(&"toolcall_end"));
    }

    /// Thinking and tool-call blocks produce their own choreography, and a
    /// toolUse stop reason maps to the done reason (faux.ts:366-421).
    #[tokio::test]
    async fn thinking_and_tool_call_blocks_stream_their_choreography() {
        let handle = faux();
        let message = faux_assistant_message(
            vec![
                faux_thinking("pondering"),
                faux_tool_call(
                    "bash",
                    serde_json::json!({"cmd": "ls"}),
                    FauxToolCallOptions::default(),
                ),
            ],
            FauxMessageOptions {
                stop_reason: Some(StopReason::ToolUse),
                ..FauxMessageOptions::default()
            },
        );
        handle.set_responses(vec![message.into()]);
        let events = drain(stream_simple(&handle, &transcript("run ls"), None)).await;

        assert_eq!(
            event_names(&events),
            [
                "start",
                "thinking_start",
                "thinking_delta",
                "thinking_end",
                "toolcall_start",
                "toolcall_delta",
                "toolcall_end",
                "done",
            ]
        );
        match &events[6] {
            AssistantMessageEvent::ToolcallEnd {
                content_index: 1,
                tool_call,
            } => {
                assert_eq!(tool_call.name, "bash");
                assert_eq!(tool_call.arguments, serde_json::json!({"cmd": "ls"}));
            }
            other => panic!("expected toolcall end, got {other:?}"),
        }
        match &events[7] {
            AssistantMessageEvent::Done { reason, .. } => {
                assert_eq!(*reason, SuccessReason::ToolUse);
            }
            other => panic!("expected done, got {other:?}"),
        }
    }

    /// An exhausted queue settles each call with the "No more faux
    /// responses queued" error and still counts the call (faux.ts:511-521).
    #[tokio::test]
    async fn exhausted_queue_yields_error_events_and_counts_calls() {
        let handle = faux();
        let events = drain(stream_simple(&handle, &transcript("hi"), None)).await;
        assert_eq!(event_names(&events), ["error"]);
        let AssistantMessageEvent::Error {
            error,
            reason: ErrorReason::Error,
        } = &events[0]
        else {
            panic!("expected error event");
        };
        assert_eq!(
            error.error_message.as_deref(),
            Some("No more faux responses queued")
        );
        assert_eq!(error.stop_reason, StopReason::Error);
        // A scripted error stop reason streams its choreography then
        // terminates with an error event (faux.ts:426-430).
        handle.set_responses(vec![faux_assistant_message(
            "boom",
            FauxMessageOptions {
                stop_reason: Some(StopReason::Error),
                error_message: Some("scripted failure".to_string()),
                ..FauxMessageOptions::default()
            },
        )
        .into()]);
        let events = drain(stream_simple(&handle, &transcript("hi"), None)).await;
        assert_eq!(
            event_names(&events),
            ["start", "text_start", "text_delta", "text_end", "error"]
        );
        let AssistantMessageEvent::Error { error, .. } = &events[4] else {
            panic!("expected error event");
        };
        assert_eq!(error.error_message.as_deref(), Some("scripted failure"));
        assert_eq!(handle.state().lock().unwrap().call_count, 2);
    }

    /// set/append replace or extend the queue; pending count tracks it
    /// (faux.ts:663-671).
    #[tokio::test]
    async fn set_and_append_manage_the_queue() {
        let handle = faux();
        assert_eq!(handle.get_pending_response_count(), 0);
        handle.set_responses(vec![
            faux_assistant_message("one", FauxMessageOptions::default()).into(),
            faux_assistant_message("two", FauxMessageOptions::default()).into(),
        ]);
        assert_eq!(handle.get_pending_response_count(), 2);
        handle.append_responses(vec![faux_assistant_message(
            "three",
            FauxMessageOptions::default(),
        )
        .into()]);
        assert_eq!(handle.get_pending_response_count(), 3);

        // FIFO order.
        for expected in ["one", "two", "three"] {
            let events = drain(stream_simple(&handle, &transcript("hi"), None)).await;
            let message = done_message(&events);
            match message.content[0] {
                AssistantBlock::Text(ref text) => assert_eq!(text.text, expected),
                ref other => panic!("unexpected block {other:?}"),
            }
        }
        assert_eq!(handle.get_pending_response_count(), 0);
    }

    /// Multi-model catalogs: getModel() default and getModel(id) lookup,
    /// with the api id unique per provider instance (faux.ts:462-486,
    /// 644-651).
    #[test]
    fn multi_model_catalog_and_lookup() {
        let handle = faux_provider(FauxProviderOptions {
            models: vec![
                FauxModelDefinition {
                    id: "m-small".to_string(),
                    name: Some("Small".to_string()),
                    context_window: Some(1_000),
                    ..FauxModelDefinition::default()
                },
                FauxModelDefinition {
                    id: "m-big".to_string(),
                    reasoning: Some(true),
                    max_tokens: Some(9_000),
                    ..FauxModelDefinition::default()
                },
            ],
            ..FauxProviderOptions::default()
        });

        assert_eq!(handle.get_model(None).unwrap().id, "m-small");
        let big = handle.get_model(Some("m-big")).unwrap();
        assert_eq!(big.name, "m-big");
        assert!(big.reasoning);
        assert_eq!(big.max_tokens, 9_000);
        assert!(handle.get_model(Some("nope")).is_none());
        // Every catalog model carries the same api/provider and the faux
        // base URL.
        for model in handle.models() {
            assert_eq!(model.api, handle.api());
            assert_eq!(model.provider, "faux");
            assert_eq!(model.base_url, "http://localhost:0");
        }
        assert_eq!(handle.provider_id(), "faux");
        // The default api id is unique per instance (randomId(DEFAULT_API)).
        let other = faux();
        assert_ne!(handle.api(), other.api());
    }

    /// Session-tagged requests get prompt-cache accounting: the first call
    /// writes the whole prompt, the second reads the common prefix
    /// (faux.ts:243-254).
    #[tokio::test]
    async fn session_usage_estimate_splits_cache_read_and_write() {
        let handle = faux();
        let session = SimpleStreamOptions {
            stream: StreamOptions {
                session_id: Some("s1".to_string()),
                ..StreamOptions::default()
            },
            ..SimpleStreamOptions::default()
        };

        handle.set_responses(vec![faux_assistant_message(
            "ok",
            FauxMessageOptions::default(),
        )
        .into()]);
        let events = drain(stream_simple(
            &handle,
            &transcript("hi"),
            Some(session.clone()),
        ))
        .await;
        let first = done_message(&events);
        assert_eq!(first.usage.input, 2);
        assert_eq!(first.usage.cache_write, 2);
        assert_eq!(first.usage.cache_read, 0);

        handle.set_responses(vec![faux_assistant_message(
            "ok",
            FauxMessageOptions::default(),
        )
        .into()]);
        let events = drain(stream_simple(
            &handle,
            &transcript("hi"),
            Some(session.clone()),
        ))
        .await;
        let second = done_message(&events);
        assert_eq!(second.usage.cache_read, 2);
        assert_eq!(second.usage.cache_write, 0);
        assert_eq!(second.usage.input, 0);

        // A different session starts over.
        let other = SimpleStreamOptions {
            stream: StreamOptions {
                session_id: Some("s2".to_string()),
                ..StreamOptions::default()
            },
            ..SimpleStreamOptions::default()
        };
        handle.set_responses(vec![faux_assistant_message(
            "ok",
            FauxMessageOptions::default(),
        )
        .into()]);
        let events = drain(stream_simple(&handle, &transcript("hi"), Some(other))).await;
        assert_eq!(done_message(&events).usage.cache_write, 2);

        // cacheRetention "none" skips the cache entirely.
        let uncached = SimpleStreamOptions {
            stream: StreamOptions {
                session_id: Some("s1".to_string()),
                cache_retention: Some(CacheRetention::None),
                ..StreamOptions::default()
            },
            ..SimpleStreamOptions::default()
        };
        handle.set_responses(vec![faux_assistant_message(
            "ok",
            FauxMessageOptions::default(),
        )
        .into()]);
        let events = drain(stream_simple(&handle, &transcript("hi"), Some(uncached))).await;
        let message = done_message(&events);
        assert_eq!(message.usage.cache_read, 0);
        assert_eq!(message.usage.cache_write, 0);
        assert_eq!(message.usage.input, 2);
    }

    /// Factory steps resolve at request time with the transcript, options,
    /// state, and request model; the async factory sees the call count
    /// incremented first (faux.ts:108-113, 161-173, 488-494).
    #[tokio::test]
    async fn factory_steps_receive_the_request_context() {
        let handle = faux();
        let factory: FauxResponseFactory = Arc::new(|args: FauxFactoryArgs| {
            Box::pin(async move {
                let call_count = args.state.lock().unwrap().call_count;
                Ok(faux_assistant_message(
                    format!("{}:{}", args.context.messages().len(), call_count),
                    FauxMessageOptions::default(),
                ))
            })
        });
        handle.set_responses(vec![FauxResponseStep::Factory(factory)]);
        let events = drain(stream_simple(&handle, &transcript("hi"), None)).await;
        // callCount incremented before the factory ran; the transcript holds
        // one message (upstream "1:1").
        let message = done_message(&events);
        match message.content[0] {
            AssistantBlock::Text(ref text) => assert_eq!(text.text, "1:1"),
            ref other => panic!("unexpected block {other:?}"),
        }
    }

    /// A thrown factory settles the stream with a single error event
    /// carrying the thrown message (faux.ts:554-558; oracle "emits an error
    /// when a response factory throws").
    #[tokio::test]
    async fn thrown_factory_becomes_a_terminal_error_event() {
        let handle = faux();
        let factory: FauxResponseFactory =
            Arc::new(|_args: FauxFactoryArgs| Box::pin(async { Err("boom".to_string()) }));
        handle.set_responses(vec![FauxResponseStep::Factory(factory)]);
        let events = drain(stream_simple(&handle, &transcript("hi"), None)).await;

        assert_eq!(events.len(), 1);
        let AssistantMessageEvent::Error {
            error,
            reason: ErrorReason::Error,
        } = &events[0]
        else {
            panic!("expected error event, got {:?}", events[0].event_type());
        };
        assert_eq!(error.stop_reason, StopReason::Error);
        assert_eq!(error.error_message.as_deref(), Some("boom"));
    }

    /// Upstream checks the `deferred` option for TRUTHINESS
    /// (`if (options?.deferred)`, faux.ts:524): `false` is an ordinary
    /// request — the message resolves immediately without a deferred handle.
    #[tokio::test]
    async fn deferred_false_is_an_ordinary_request() {
        let handle = faux();
        handle.set_responses(vec![faux_assistant_message(
            "final",
            FauxMessageOptions::default(),
        )
        .into()]);
        let deferred_options = SimpleStreamOptions {
            deferred: Some(DeferredFlag::Bool(false)),
            ..SimpleStreamOptions::default()
        };
        let events = drain(stream_simple(
            &handle,
            &transcript("hi"),
            Some(deferred_options),
        ))
        .await;
        // An ordinary request streams the full text choreography and
        // resolves immediately — no deferred handle, no deferred bookkeeping.
        assert_eq!(
            event_names(&events),
            ["start", "text_start", "text_delta", "text_end", "done"]
        );
        let message = done_message(&events);
        assert_eq!(message.stop_reason, StopReason::Stop);
        assert!(message.deferred.is_none());
        assert_eq!(handle.state().lock().unwrap().deferred_fetch_count, 0);
    }

    /// Deferred requests: the stream settles with the handle, pending
    /// fetches replay the handle, then the final response resolves; cancel
    /// records the handle and fails later fetches (faux.ts:524-550,
    /// 567-642).
    #[tokio::test]
    async fn deferred_requests_stream_handle_then_final_response() {
        let handle = faux_provider(FauxProviderOptions {
            deferred: Some(FauxDeferredOptions {
                pending_fetches: Some(1),
                poll_after_ms: Some(250),
            }),
            ..FauxProviderOptions::default()
        });
        handle.set_responses(vec![faux_assistant_message(
            "final",
            FauxMessageOptions::default(),
        )
        .into()]);
        let deferred_options = SimpleStreamOptions {
            deferred: Some(DeferredFlag::Bool(true)),
            ..SimpleStreamOptions::default()
        };
        let events = drain(stream_simple(
            &handle,
            &transcript("hi"),
            Some(deferred_options),
        ))
        .await;
        assert_eq!(event_names(&events), ["start", "done"]);
        let handle_out = done_message(&events)
            .deferred
            .clone()
            .expect("deferred message carries the handle");
        assert_eq!(handle_out.poll_after_ms, Some(250));
        assert_eq!(handle_out.api, handle.api());
        assert_eq!(handle_out.model_id, "faux-1");

        // First fetch: pendingFetches 1 -> replays the deferred message.
        let model = handle.get_model(None).unwrap();
        let events = drain(handle.fetch_deferred(&model, &handle_out)).await;
        assert_eq!(event_names(&events), ["start", "done"]);
        assert!(done_message(&events).deferred.is_some());
        assert_eq!(handle.state().lock().unwrap().deferred_fetch_count, 1);

        // Second fetch: resolves the final scripted response.
        let events = drain(handle.fetch_deferred(&model, &handle_out)).await;
        assert_eq!(
            event_names(&events),
            ["start", "text_start", "text_delta", "text_end", "done"]
        );
        assert!(done_message(&events).deferred.is_none());
        assert_eq!(
            match done_message(&events).content[0] {
                AssistantBlock::Text(ref text) => text.text.clone(),
                ref other => panic!("unexpected block {other:?}"),
            },
            "final"
        );

        // Cancel: records the handle and fails subsequent fetches.
        handle.cancel_deferred(&handle_out).await;
        assert_eq!(handle.state().lock().unwrap().cancelled_deferred.len(), 1);
        let events = drain(handle.fetch_deferred(&model, &handle_out)).await;
        let AssistantMessageEvent::Error { error, .. } = &events[0] else {
            panic!("expected error event");
        };
        assert!(
            error
                .error_message
                .as_deref()
                .unwrap_or_default()
                .starts_with("Faux deferred response was cancelled:"),
            "{error:?}"
        );

        // Unknown handles fail too.
        let ghost = DeferredHandle {
            provider: "faux".to_string(),
            model_id: "faux-1".to_string(),
            api: handle.api().to_string(),
            id: "nope".to_string(),
            expires_at: None,
            poll_after_ms: None,
            data: None,
        };
        let events = drain(handle.fetch_deferred(&model, &ghost)).await;
        let AssistantMessageEvent::Error { error, .. } = &events[0] else {
            panic!("expected error event");
        };
        assert_eq!(
            error.error_message.as_deref(),
            Some("Unknown faux deferred response: nope")
        );
    }

    /// `fauxProvider` builds a Provider usable from a `Models` collection:
    /// ambient auth, static catalog, and scripted streams through the
    /// routed surface (faux.ts:685-708).
    #[tokio::test]
    async fn faux_provider_registers_with_a_models_collection() {
        let faux = faux();
        let mut models =
            super::super::create_models(crate::ai::models::CreateModelsOptions::default());
        models.set_provider(Arc::clone(&faux.provider));

        let model = models
            .get_model("faux", "faux-1")
            .expect("faux model visible");
        assert_eq!(model.api, faux.api());
        faux.set_responses(vec![faux_assistant_message(
            "routed",
            FauxMessageOptions::default(),
        )
        .into()]);
        let events = drain(models.stream(
            &model,
            &crate::ai::Context {
                system_prompt: None,
                messages: vec![Message::User(UserMessage {
                    content: StringOrBlocks::Text("hi".to_string()),
                    timestamp: 1,
                })],
                tools: None,
            },
            None,
        ))
        .await;
        let message = match events.last().expect("routed stream produced events") {
            AssistantMessageEvent::Done { message, .. } => message.clone(),
            other => panic!("expected done, got {other:?}"),
        };
        match message.content[0] {
            AssistantBlock::Text(ref text) => assert_eq!(text.text, "routed"),
            ref other => panic!("unexpected block {other:?}"),
        }
        // Ambient api-key auth reports configured for get_available.
        assert!(faux.provider.auth().api_key.is_some());
        assert_eq!(
            faux.provider.auth().api_key.as_ref().unwrap().name(),
            "Faux"
        );
    }

    /// The faux helpers build mixed content verbatim; a toolUse stop reason
    /// rides through (oracle "supports helper blocks for text, thinking, and
    /// tool calls", faux.ts:53-68).
    #[tokio::test]
    async fn helper_blocks_build_mixed_content() {
        let handle = faux();
        handle.set_responses(vec![faux_assistant_message(
            vec![
                faux_thinking("think"),
                faux_tool_call(
                    "echo",
                    serde_json::json!({"text": "hi"}),
                    FauxToolCallOptions::default(),
                ),
                faux_text("done"),
            ],
            FauxMessageOptions {
                stop_reason: Some(StopReason::ToolUse),
                ..FauxMessageOptions::default()
            },
        )
        .into()]);
        let events = drain(stream_simple(&handle, &transcript("hi"), None)).await;

        let message = done_message(&events);
        assert_eq!(message.content.len(), 3);
        match &message.content[0] {
            AssistantBlock::Thinking(thinking) => assert_eq!(thinking.thinking, "think"),
            ref other => panic!("unexpected block {other:?}"),
        }
        match &message.content[1] {
            AssistantBlock::ToolCall(tool_call) => {
                assert!(!tool_call.id.is_empty());
                assert_eq!(tool_call.name, "echo");
                assert_eq!(tool_call.arguments, serde_json::json!({"text": "hi"}));
            }
            ref other => panic!("unexpected block {other:?}"),
        }
        match &message.content[2] {
            AssistantBlock::Text(text) => assert_eq!(text.text, "done"),
            ref other => panic!("unexpected block {other:?}"),
        }
        assert_eq!(message.stop_reason, StopReason::ToolUse);
    }

    /// Returned messages are re-stamped with the request's api, provider,
    /// and model (faux.ts:281-291, 494-500; oracle "rewrites api, provider,
    /// and model on returned messages").
    #[tokio::test]
    async fn returned_messages_are_rewritten_to_the_request_identity() {
        let handle = faux_provider(FauxProviderOptions {
            api: Some("faux:test".to_string()),
            provider: Some("faux-provider".to_string()),
            models: vec![FauxModelDefinition {
                id: "faux-model".to_string(),
                ..FauxModelDefinition::default()
            }],
            ..FauxProviderOptions::default()
        });
        handle.set_responses(vec![faux_assistant_message(
            "hello",
            FauxMessageOptions::default(),
        )
        .into()]);
        let events = drain(stream_simple(&handle, &transcript("hi"), None)).await;

        let message = done_message(&events);
        assert_eq!(message.api, "faux:test");
        assert_eq!(message.provider, "faux-provider");
        assert_eq!(message.model, "faux-model");
    }

    /// Multiple tool calls in one message each stream a full toolcall
    /// choreography (faux.ts:356-421; oracle "streams multiple tool calls in
    /// one message").
    #[tokio::test]
    async fn multiple_tool_calls_stream_in_one_message() {
        let handle = faux();
        handle.set_responses(vec![faux_assistant_message(
            vec![
                faux_tool_call(
                    "echo",
                    serde_json::json!({"text": "one"}),
                    FauxToolCallOptions {
                        id: Some("tool-1".to_string()),
                    },
                ),
                faux_tool_call(
                    "echo",
                    serde_json::json!({"text": "two"}),
                    FauxToolCallOptions {
                        id: Some("tool-2".to_string()),
                    },
                ),
            ],
            FauxMessageOptions {
                stop_reason: Some(StopReason::ToolUse),
                ..FauxMessageOptions::default()
            },
        )
        .into()]);
        let events = drain(stream_simple(&handle, &transcript("hi"), None)).await;

        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, AssistantMessageEvent::ToolcallStart { .. }))
                .count(),
            2
        );
        let ends: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                AssistantMessageEvent::ToolcallEnd { tool_call, .. } => Some(tool_call.id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(ends, ["tool-1", "tool-2"]);
    }

    /// With one-token chunking every block emits exactly one delta and the
    /// event order is fixed; the start event carries the pending partial
    /// (faux.ts:346, 354-421; oracle "streams an exact event order for
    /// fixed-size chunks").
    #[tokio::test]
    async fn fixed_size_chunks_stream_the_exact_event_order() {
        let handle = faux_provider(FauxProviderOptions {
            token_size: Some(FauxTokenSize {
                min: Some(1),
                max: Some(1),
            }),
            ..FauxProviderOptions::default()
        });
        handle.set_responses(vec![faux_assistant_message(
            vec![
                faux_thinking("go"),
                faux_text("ok"),
                faux_tool_call(
                    "echo",
                    serde_json::json!({}),
                    FauxToolCallOptions {
                        id: Some("tool-1".to_string()),
                    },
                ),
            ],
            FauxMessageOptions {
                stop_reason: Some(StopReason::ToolUse),
                ..FauxMessageOptions::default()
            },
        )
        .into()]);
        let events = drain(stream_simple(&handle, &transcript("hi"), None)).await;

        match &events[0] {
            AssistantMessageEvent::Start { message } => {
                assert_eq!(message.stop_reason, StopReason::Pending);
            }
            other => panic!("expected start event, got {other:?}"),
        }
        assert_eq!(
            event_names(&events),
            [
                "start",
                "thinking_start",
                "thinking_delta",
                "thinking_end",
                "text_start",
                "text_delta",
                "text_end",
                "toolcall_start",
                "toolcall_delta",
                "toolcall_end",
                "done",
            ]
        );
    }

    /// A queued response without a terminal stop reason never emits done;
    /// the stream settles with the contract-violation error (faux.ts:423-425;
    /// oracle "rejects a queued response without a terminal stop reason").
    #[tokio::test]
    async fn pending_stop_reason_rejects_with_the_contract_error() {
        let handle = faux();
        handle.set_responses(vec![faux_assistant_message(
            "partial",
            FauxMessageOptions {
                stop_reason: Some(StopReason::Pending),
                ..FauxMessageOptions::default()
            },
        )
        .into()]);
        let events = drain(stream_simple(&handle, &transcript("hi"), None)).await;

        assert!(!events
            .iter()
            .any(|event| matches!(event, AssistantMessageEvent::Done { .. })));
        let AssistantMessageEvent::Error { error, .. } = events.last().unwrap() else {
            panic!("expected terminal error, got {:?}", events.last().unwrap());
        };
        assert_eq!(error.stop_reason, StopReason::Error);
        assert_eq!(
            error.error_message.as_deref(),
            Some("Faux response ended without a stop reason")
        );
    }

    /// A scripted aborted message streams its choreography and terminates
    /// with an aborted error event (faux.ts:426-430; oracle "streams an
    /// explicit assistant aborted message as a terminal error").
    #[tokio::test]
    async fn scripted_aborted_message_streams_a_terminal_aborted_error() {
        let handle = faux_provider(FauxProviderOptions {
            token_size: Some(FauxTokenSize {
                min: Some(2),
                max: Some(2),
            }),
            ..FauxProviderOptions::default()
        });
        handle.set_responses(vec![faux_assistant_message(
            "partial",
            FauxMessageOptions {
                stop_reason: Some(StopReason::Aborted),
                error_message: Some("Request was aborted".to_string()),
                ..FauxMessageOptions::default()
            },
        )
        .into()]);
        let events = drain(stream_simple(&handle, &transcript("hi"), None)).await;

        assert_eq!(
            event_names(&events),
            ["start", "text_start", "text_delta", "text_end", "error"]
        );
        let AssistantMessageEvent::Error { error, reason } = events.last().unwrap() else {
            panic!("expected terminal error, got {:?}", events.last().unwrap());
        };
        assert_eq!(*reason, ErrorReason::Aborted);
        assert_eq!(error.stop_reason, StopReason::Aborted);
        assert_eq!(error.error_message.as_deref(), Some("Request was aborted"));
    }

    /// Session-tagged requests: a different session starts a fresh prompt
    /// cache, and requests without a sessionId never touch the cache
    /// (faux.ts:241-254; oracle "does not share cache across sessions or
    /// requests without sessionId").
    #[tokio::test]
    async fn cache_does_not_share_across_sessions_or_without_session_id() {
        let handle = faux();
        let mut messages = vec![Message::User(UserMessage {
            content: StringOrBlocks::Text("hello".to_string()),
            timestamp: 1758240000000,
        })];
        let context_for = |messages: Vec<Message>| {
            normalize_context(&crate::ai::Context {
                system_prompt: None,
                messages,
                tools: None,
            })
        };
        let options_for = |session_id: &str| SimpleStreamOptions {
            stream: StreamOptions {
                session_id: Some(session_id.to_string()),
                cache_retention: Some(CacheRetention::Short),
                ..StreamOptions::default()
            },
            ..SimpleStreamOptions::default()
        };

        handle.set_responses(vec![faux_assistant_message(
            "first",
            FauxMessageOptions::default(),
        )
        .into()]);
        let events = drain(stream_simple(
            &handle,
            &context_for(messages.clone()),
            Some(options_for("session-1")),
        ))
        .await;
        let first = done_message(&events).clone();
        assert!(first.usage.cache_write > 0);
        messages.push(Message::Assistant(first));
        messages.push(Message::User(UserMessage {
            content: StringOrBlocks::Text("follow up".to_string()),
            timestamp: 1758240000001,
        }));

        handle.set_responses(vec![faux_assistant_message(
            "second",
            FauxMessageOptions::default(),
        )
        .into()]);
        let events = drain(stream_simple(
            &handle,
            &context_for(messages.clone()),
            Some(options_for("session-2")),
        ))
        .await;
        let second = done_message(&events);
        assert_eq!(second.usage.cache_read, 0);
        assert!(second.usage.cache_write > 0);

        handle.set_responses(vec![faux_assistant_message(
            "third",
            FauxMessageOptions::default(),
        )
        .into()]);
        let events = drain(stream_simple(&handle, &context_for(messages), None)).await;
        let third = done_message(&events);
        assert_eq!(third.usage.cache_read, 0);
        assert_eq!(third.usage.cache_write, 0);
    }

    /// Usage token-estimates the serialized transcript (oracle "estimates
    /// prompt and output tokens from serialized context"): the expected
    /// prompt mirrors the oracle's line construction — the faux serializer
    /// inlines tool additions into the system message (`tool+:`) where the
    /// oracle appends a `tools:` line, and the two renderings have equal
    /// length in the oracle, so the token counts agree (281 vs 284 chars,
    /// both ceil to 71).
    #[tokio::test]
    async fn usage_estimates_the_serialized_context() {
        let handle = faux();
        handle.set_responses(vec![faux_assistant_message(
            "done",
            FauxMessageOptions::default(),
        )
        .into()]);

        let tool = Tool {
            name: "echo".to_string(),
            description: "Echo back text".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"text": {"type": "string"}},
                "required": ["text"],
                "$schema": "http://json-schema.org/draft-07/schema#",
            }),
            constrained_sampling: None,
        };
        let context = normalize_context(&crate::ai::Context {
            system_prompt: Some("sys".to_string()),
            messages: vec![
                Message::User(UserMessage {
                    content: StringOrBlocks::Blocks(vec![
                        TextOrImageBlock::Text(TextContent {
                            text: "hello".to_string(),
                            text_signature: None,
                        }),
                        TextOrImageBlock::Image(ImageContent {
                            data: "abcd".to_string(),
                            mime_type: "image/png".to_string(),
                        }),
                    ]),
                    timestamp: 1,
                }),
                Message::Assistant(faux_assistant_message(
                    "prior",
                    FauxMessageOptions {
                        timestamp: Some(0),
                        ..FauxMessageOptions::default()
                    },
                )),
                Message::ToolResult(ToolResultMessage {
                    tool_call_id: "tool-1".to_string(),
                    tool_name: "echo".to_string(),
                    content: vec![TextOrImageBlock::Text(TextContent {
                        text: "tool out".to_string(),
                        text_signature: None,
                    })],
                    details: None,
                    usage: None,
                    is_error: false,
                    timestamp: 2,
                }),
            ],
            tools: Some(vec![tool.clone()]),
        });

        let events = drain(stream_simple(&handle, &context, None)).await;
        let message = done_message(&events);
        let expected_prompt = [
            "system:sys",
            "user:hello\n[image:image/png:4]",
            "assistant:prior",
            "toolResult:echo\ntool out",
            &format!(
                "tools:{}",
                serde_json::to_string(&vec![tool.clone()]).unwrap()
            ),
        ]
        .join("\n\n");
        let expected_prompt_tokens = estimate_tokens(&expected_prompt);
        let expected_output_tokens = estimate_tokens("done");

        assert_eq!(message.usage.input, expected_prompt_tokens as u64);
        assert_eq!(message.usage.output, expected_output_tokens as u64);
        assert_eq!(message.usage.cache_read, 0);
        assert_eq!(message.usage.cache_write, 0);
        assert_eq!(
            message.usage.total_tokens,
            (expected_prompt_tokens + expected_output_tokens) as u64
        );
        // Oracle "registers a custom provider and estimates usage": one call
        // counted for the request.
        assert_eq!(handle.state().lock().unwrap().call_count, 1);
    }

    /// splitStringByTokenSize: chunk concatenation preserves the text and
    /// every chunk respects the size cap (faux.ts:269-279).
    #[test]
    fn split_string_by_token_size_preserves_text() {
        let text = "abcdefghij".repeat(10);
        let chunks = split_string_by_token_size(&text, 3, 5);
        let joined: String = chunks.concat();
        assert_eq!(joined, text);
        // Chars per chunk: random token size in 3..=5, times four.
        for chunk in &chunks {
            assert!(chunk.chars().count() <= 5 * 4);
        }
        // Empty input still yields one empty chunk.
        assert_eq!(split_string_by_token_size("", 3, 5), vec![""]);
    }

    /// to_base36 matches JS number-to-base36 digit rendering.
    #[test]
    fn base36_digits_match_js() {
        assert_eq!(to_base36(0), "0");
        assert_eq!(to_base36(35), "z");
        assert_eq!(to_base36(36), "10");
        assert_eq!(to_base36(46656), "1000");
    }

    /// estimateTokens: ceil(chars / 4).
    #[test]
    fn estimate_tokens_rounds_up() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abc"), 1);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
    }
}
