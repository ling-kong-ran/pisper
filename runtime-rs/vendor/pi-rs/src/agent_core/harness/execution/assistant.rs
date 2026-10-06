//! Port of `packages/agent/src/harness/execution/assistant.ts` (175 lines):
//! stream one assistant response through the caller-supplied request
//! boundary — curated request options, the observer lifecycle, and the
//! after-response settlement hook.
//!
//! Disclosed substitutions:
//! - **Event stream.** Upstream `AssistantMessageEventStream` is an async
//!   iterable with a `result()` promise; the port's ai layer delivers
//!   [`AssistantMessageEvent`]s through a
//!   `tokio::sync::mpsc::Receiver` and the terminal event (`done`/`error`)
//!   carries the settled message. [`consume_assistant_stream`] reads the
//!   settled message from that event. A channel that closes without a
//!   terminal event errors instead of hanging (upstream's `stream.result()`
//!   would never settle; the port surfaces the protocol violation — the same
//!   defensive posture as the agent-loop port's synthesized stream error).
//! - **Live partial.** Upstream events carry the shared `partial`; the port
//!   reconstructs it with
//!   [`PartialAssistant`](crate::ai::types::events::PartialAssistant) (the
//!   ai-layer substitution, documented there).
//! - **Request options.** Upstream `createRequestOptions` returns
//!   `SimpleStreamOptions` including `onPayload`/`onResponse` and
//!   `telemetryContext`. [`AssistantRequestOptions`] keeps context-aware
//!   callbacks; generation bridges them into the ai-layer process-local
//!   `RequestCallbacks`. Telemetry context is still unported. The port's
//!   `onPayload` also receives the `Context` (upstream
//!   captures it in the closure) so callers can invoke it directly.
//! - **`afterResponse` errors.** Upstream rethrows non-`AbortRequested`
//!   errors and awaits `error.cancellation` for the abort kind; the port
//!   recognizes both [`AbortRequested`] and the gate's typed abort refusal,
//!   then awaits the cancellation token before keeping the raw settlement.
//! - **Headers.** `AssistantResponseMetadata.headers` is a
//!   `BTreeMap<String, String>` (upstream `Record<string, string>`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use tokio::sync::mpsc;

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::execution::effect_gate::AbortRequested;
use crate::agent_core::harness::hooks::SettledAssistantMessage;
use crate::agent_core::harness::types::AgentHarnessStreamOptions;
use crate::agent_core::types::{AgentMessage, ThinkingLevel};
use crate::ai::types::events::{AssistantMessageEvent, PartialAssistant};
use crate::ai::types::message::{AssistantMessage, Message};
use crate::ai::types::model::Model;
use crate::ai::types::options::{SimpleStreamOptions, StreamOptions};
use crate::ai::types::primitives::ThinkingLevel as RequestThinkingLevel;
use crate::ai::types::tool::Tool;

/// The port's assistant event stream: upstream `AssistantMessageEventStream`
/// (a `Receiver` of the ai-layer events; the terminal event carries the
/// settled message).
pub type AssistantMessageEventStream = mpsc::Receiver<AssistantMessageEvent>;

/// Upstream `AiContext` (`assistant.ts:2`, `@earendil-works/pi-ai` `Context`):
/// the request input handed to the `request` boundary.
pub type AiContext = crate::ai::transcript::Context;

/// HTTP response metadata captured before the provider response body is
/// consumed (upstream `AssistantResponseMetadata`, `assistant.ts:19-22`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AssistantResponseMetadata {
    pub status: Option<u16>,
    pub headers: Option<BTreeMap<String, String>>,
}

/// The `(payload, model, context) => unknown | undefined` before-payload
/// callback (upstream `beforePayload`, `assistant.ts:47-51`); `None` keeps
/// the payload, `Some` replaces it.
pub type PayloadCallback = dyn Fn(
        serde_json::Value,
        Model,
        Context,
    ) -> BoxFuture<'static, anyhow::Result<Option<serde_json::Value>>>
    + Send
    + Sync;

/// The `onResponse` capture callback (upstream `assistant.ts:87-89`): the
/// request options deliver the status/headers pair before the body is read.
pub type ResponseCallback = dyn Fn(AssistantResponseMetadata) + Send + Sync;

/// The settled-message transform (upstream `afterResponse`,
/// `assistant.ts:52-56`). An [`AbortRequested`] error takes the
/// keep-the-raw-settlement path; other errors propagate.
pub type AfterResponseFn = dyn Fn(
        SettledAssistantMessage,
        AssistantResponseMetadata,
        Context,
    ) -> BoxFuture<'static, anyhow::Result<SettledAssistantMessage>>
    + Send
    + Sync;

/// The `afterResponse` argument of upstream `consumeAssistantStream`
/// (`assistant.ts:102-104`): the config hook already fused with the captured
/// response metadata (upstream `assistant.ts:166-172`).
pub type SettleTransformFn = dyn Fn(
        SettledAssistantMessage,
        Context,
    ) -> BoxFuture<'static, anyhow::Result<SettledAssistantMessage>>
    + Send
    + Sync;

/// The request boundary (upstream `config.request`, `assistant.ts:57-61`).
pub type AssistantRequestFn = dyn Fn(
        AiContext,
        AssistantRequestOptions,
        Context,
    ) -> BoxFuture<'static, anyhow::Result<AssistantMessageEventStream>>
    + Send
    + Sync;

/// Upstream `SimpleStreamOptions` for one request plus the two callback
/// context-aware callback fields (generation bridges these to the ai-layer
/// callbacks; upstream carries them on `SimpleStreamOptions` directly).
#[derive(Clone, Default)]
pub struct AssistantRequestOptions {
    /// The ai-layer simple stream options (transport, timeouts, headers,
    /// metadata, cache retention, deferred, reasoning, signal).
    pub simple: SimpleStreamOptions,
    /// Upstream `onPayload` (`assistant.ts:83-86`): runs once over the
    /// assembled provider request body; `Some` replaces it.
    pub on_payload: Option<Arc<PayloadCallback>>,
    /// Upstream `onResponse` (`assistant.ts:87-89`): captures response
    /// metadata before the body is consumed.
    pub on_response: Option<Arc<ResponseCallback>>,
}

/// Process-local lifecycle observer for one assistant stream (upstream
/// `AssistantStreamObserver`, `assistant.ts:25-33`). Methods are async like
/// upstream (they may await); the port boxes their futures.
pub trait AssistantStreamObserver: Send + Sync {
    /// Upstream `start(message, event, context)`.
    fn start<'a>(
        &'a self,
        message: AssistantMessage,
        event: AssistantMessageEvent,
        context: Context,
    ) -> BoxFuture<'a, ()>;
    /// Upstream `update(message, event, context)` — every non-terminal event
    /// after `start`.
    fn update<'a>(
        &'a self,
        message: AssistantMessage,
        event: AssistantMessageEvent,
        context: Context,
    ) -> BoxFuture<'a, ()>;
    /// Upstream `end(message, context)` — the final (possibly patched)
    /// settlement.
    fn end<'a>(&'a self, message: SettledAssistantMessage, context: Context) -> BoxFuture<'a, ()>;

    // Fallible observer entry points mirror rejected TS observer promises.
    // Default wrappers retain the existing infallible Rust observer API.
    fn try_start<'a>(
        &'a self,
        message: AssistantMessage,
        event: AssistantMessageEvent,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            self.start(message, event, context).await;
            Ok(())
        })
    }
    fn try_update<'a>(
        &'a self,
        message: AssistantMessage,
        event: AssistantMessageEvent,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            self.update(message, event, context).await;
            Ok(())
        })
    }
    fn try_end<'a>(
        &'a self,
        message: SettledAssistantMessage,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            self.end(message, context).await;
            Ok(())
        })
    }
}

/// Upstream `transformContext` (`assistant.ts:42-45`): an infallible
/// transform over the request context.
pub type TransformContextFn = dyn Fn(HarnessRequestContext, Context) -> BoxFuture<'static, anyhow::Result<HarnessRequestContext>>
    + Send
    + Sync;

/// Upstream `toProviderMessages` (`assistant.ts:46`): the transcript-to-LLM
/// conversion (infallible upstream).
pub type ToProviderMessagesFn =
    dyn Fn(Vec<AgentMessage>, Context) -> BoxFuture<'static, Vec<Message>> + Send + Sync;

/// The `{ messages, systemPrompt }` request context upstream threads through
/// `transformContext` (`assistant.ts:42-45`).
#[derive(Debug, Clone, PartialEq)]
pub struct HarnessRequestContext {
    pub messages: Vec<AgentMessage>,
    pub system_prompt: String,
}

/// Executable inputs for one already-approved assistant provider request
/// (upstream `HarnessAssistantStreamConfig`, `assistant.ts:36-63`).
pub struct HarnessAssistantStreamConfig {
    pub model: Model,
    pub system_prompt: String,
    pub tools: Option<Vec<Tool>>,
    pub thinking_level: ThinkingLevel,
    pub stream_options: AgentHarnessStreamOptions,
    /// Upstream `transformContext`: an async transform over the request
    /// context; it cannot fail upstream, so the port's callback is
    /// infallible.
    pub transform_context: Option<Arc<TransformContextFn>>,
    /// Upstream `toProviderMessages`: the transcript-to-LLM conversion
    /// (infallible upstream).
    pub to_provider_messages: Arc<ToProviderMessagesFn>,
    pub before_payload: Option<Arc<PayloadCallback>>,
    pub after_response: Option<Arc<AfterResponseFn>>,
    pub request: Arc<AssistantRequestFn>,
    pub observer: Arc<dyn AssistantStreamObserver>,
}

/// Upstream `createRequestOptions` (`assistant.ts:65-91`): map the curated
/// harness options onto the provider request options, attach the abort
/// signal, reasoning level (everything but `"off"`), the payload callback,
/// and the response-metadata capture.
fn create_request_options(
    config: &HarnessAssistantStreamConfig,
    metadata: &Arc<Mutex<Option<AssistantResponseMetadata>>>,
    context: &Context,
) -> AssistantRequestOptions {
    let options = &config.stream_options;
    let reasoning = match config.thinking_level {
        ThinkingLevel::Off => None,
        ThinkingLevel::Minimal => Some(RequestThinkingLevel::Minimal),
        ThinkingLevel::Low => Some(RequestThinkingLevel::Low),
        ThinkingLevel::Medium => Some(RequestThinkingLevel::Medium),
        ThinkingLevel::High => Some(RequestThinkingLevel::High),
        ThinkingLevel::Xhigh => Some(RequestThinkingLevel::Xhigh),
        ThinkingLevel::Max => Some(RequestThinkingLevel::Max),
    };
    let simple = SimpleStreamOptions {
        stream: StreamOptions {
            transport: options.transport,
            timeout_ms: options.timeout_ms,
            max_retries: options.max_retries,
            max_retry_delay_ms: options.max_retry_delay_ms,
            headers: options.headers.clone().map(|headers| {
                headers
                    .into_iter()
                    .map(|(key, value)| (key, Some(value)))
                    .collect()
            }),
            metadata: options.metadata.clone(),
            cache_retention: options.cache_retention,
            signal: context.abort_signal(),
            ..StreamOptions::default()
        },
        deferred: options.deferred,
        reasoning,
        ..SimpleStreamOptions::default()
    };
    AssistantRequestOptions {
        simple,
        on_payload: config.before_payload.clone(),
        on_response: Some(Arc::new({
            let metadata = Arc::clone(metadata);
            move |response: AssistantResponseMetadata| {
                *metadata.lock().unwrap() = Some(response);
            }
        })),
    }
}

/// Upstream `consumeAssistantStream` (`assistant.ts:99-133`): drive one
/// event stream through the observer lifecycle, take the settled message
/// from the terminal event, run the after-response hook (with the
/// `AbortRequested` keep-raw path), and close with `observer.end`.
pub async fn consume_assistant_stream(
    mut stream: AssistantMessageEventStream,
    observer: Arc<dyn AssistantStreamObserver>,
    after_response: Option<Arc<SettleTransformFn>>,
    context: Context,
) -> anyhow::Result<SettledAssistantMessage> {
    let mut started = false;
    let mut settled: Option<AssistantMessage> = None;
    // The live partial (upstream events carry `event.partial`).
    let mut partial = PartialAssistant::new();
    while let Some(event) = stream.recv().await {
        match &event {
            AssistantMessageEvent::Start { message } => {
                if started {
                    anyhow::bail!("Assistant message stream emitted more than one start event");
                }
                started = true;
                let _ = partial.apply(&event);
                observer
                    .try_start(message.clone(), event.clone(), context.clone())
                    .await?;
            }
            AssistantMessageEvent::Done { message, .. } => {
                if !started {
                    anyhow::bail!("Assistant message stream emitted done before start");
                }
                let _ = partial.apply(&event);
                settled = Some(message.clone());
            }
            // Upstream ignores error events inside the loop (`isUpdateEvent`);
            // the settled message comes from `stream.result()`, which for an
            // error settlement is the error message. Pre-start errors are
            // allowed (assistant.ts:116 only rejects `done` before start).
            AssistantMessageEvent::Error { error, .. } => {
                let _ = partial.apply(&event);
                settled = Some(error.clone());
            }
            other => {
                if !started {
                    anyhow::bail!(
                        "Assistant message stream emitted {} before start",
                        other.event_type()
                    );
                }
                let _ = partial.apply(other);
                if let Some(snapshot) = partial.message().cloned() {
                    observer
                        .try_update(snapshot, other.clone(), context.clone())
                        .await?;
                }
            }
        }
    }
    let settled = settled.ok_or_else(|| {
        anyhow::anyhow!("Assistant message stream closed without a terminal event")
    })?;
    let mut final_message = settled.clone();
    if let Some(after) = after_response {
        match after(settled.clone(), context.clone()).await {
            Ok(message) => final_message = message,
            Err(error) => {
                let cancellation = error
                    .downcast_ref::<AbortRequested>()
                    .map(|a| a.cancellation.clone())
                    .or_else(
                        || match error.downcast_ref::<super::effect_gate::GateRejection>() {
                            Some(super::effect_gate::GateRejection::AbortRequested(a)) => {
                                Some(a.cancellation.clone())
                            }
                            _ => None,
                        },
                    );
                if let Some(cancellation) = cancellation {
                    cancellation.cancelled().await;
                } else {
                    return Err(error);
                }
            }
        }
    }
    observer.try_end(final_message.clone(), context).await?;
    Ok(final_message)
}

/// Upstream `streamHarnessAssistant` (`assistant.ts:136-175`): stream one
/// assistant response without mutating the caller's message list.
pub async fn stream_harness_assistant(
    messages: &[AgentMessage],
    config: &HarnessAssistantStreamConfig,
    context: Context,
) -> anyhow::Result<SettledAssistantMessage> {
    // `messages.slice()` — the transform only sees a copy.
    let mut request_context = HarnessRequestContext {
        messages: messages.to_vec(),
        system_prompt: config.system_prompt.clone(),
    };
    if let Some(transform) = &config.transform_context {
        request_context = transform(request_context, context.clone()).await?;
    }

    let provider_messages =
        (config.to_provider_messages)(request_context.messages.clone(), context.clone()).await;
    let ai_context = AiContext {
        system_prompt: Some(request_context.system_prompt.clone()),
        messages: provider_messages,
        tools: config.tools.clone(),
    };

    let metadata: Arc<Mutex<Option<AssistantResponseMetadata>>> = Arc::new(Mutex::new(None));
    let options = create_request_options(config, &metadata, &context);
    let stream = (config.request)(ai_context, options, context.clone()).await?;

    // `afterResponse` sees the metadata captured during the request
    // (assistant.ts:166-172): the config hook fuses with the captured
    // metadata into the 2-argument transform `consumeAssistantStream` runs.
    let wrapped_after = config.after_response.clone().map(|after| {
        let metadata = Arc::clone(&metadata);
        Arc::new(move |message: SettledAssistantMessage, context: Context| {
            let after = Arc::clone(&after);
            let metadata = Arc::clone(&metadata);
            Box::pin(async move {
                let captured = metadata.lock().unwrap().clone().unwrap_or_default();
                after(message, captured, context).await
            }) as BoxFuture<'static, anyhow::Result<SettledAssistantMessage>>
        }) as Arc<SettleTransformFn>
    });
    consume_assistant_stream(stream, Arc::clone(&config.observer), wrapped_after, context).await
}

#[cfg(test)]
mod tests;
