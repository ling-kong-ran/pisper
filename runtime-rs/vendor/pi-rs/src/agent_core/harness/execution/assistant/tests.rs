//! Port of `packages/agent/test/harness/execution-assistant.test.ts` (the
//! executable spec for the assistant stream driver). The manual-stream cases
//! build the port's `AssistantMessageEventStream` (an mpsc receiver whose
//! terminal event carries the settled message); the faux-provider case runs
//! the real ai-layer request boundary.

use std::sync::{Arc, Mutex};

use super::*;
use crate::agent_core::harness::context::{background_context, with_abort_signal};
use crate::agent_core::types::AgentMessage;
use crate::ai::models::faux::{
    faux_assistant_message, faux_provider, FauxMessageOptions, FauxProviderOptions, FauxTokenSize,
};
use crate::ai::models::{create_models, CreateModelsOptions, ModelsSimpleStreamOptions};
use crate::ai::types::events::{AssistantMessageEvent, ErrorReason, SuccessReason};
use crate::ai::types::message::{
    AssistantBlock, AssistantMessage, Message, StringOrBlocks, UserMessage,
};
use crate::ai::types::model::{Model, ModelInput};
use crate::ai::types::options::{DeferredFlag, DeferredWindow};
use crate::ai::types::primitives::ThinkingLevel as RequestThinkingLevel;
use crate::ai::types::primitives::{CacheRetention, ModelCost, StopReason, Transport, Usage};
use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

/// The oracle's `usage()` fixture (`execution-assistant.test.ts:20-29`).
fn usage() -> Usage {
    Usage {
        input: 1,
        output: 2,
        cache_read: 3,
        cache_write: 4,
        cache_write_1h: None,
        reasoning: None,
        total_tokens: 10,
        cost: Default::default(),
    }
}

/// The oracle's `model()` fixture (`execution-assistant.test.ts:31-44`).
fn model() -> Model {
    Model {
        id: "model".into(),
        name: "Model".into(),
        api: "test".into(),
        provider: "provider".into(),
        base_url: "https://example.invalid".into(),
        reasoning: true,
        thinking_level_map: None,
        input: vec![ModelInput::Text],
        cost: ModelCost::default(),
        context_window: 128_000,
        max_tokens: 16_384,
        sampling_params: None,
        sampling_params_by_thinking_level: None,
        headers: None,
        compat: None,
        r#type: None,
        prompt_cache: None,
        input_limits: None,
    }
}

/// The oracle's `user()` fixture (`execution-assistant.test.ts:46-48`).
fn user(text: &str) -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: StringOrBlocks::Text(text.into()),
        timestamp: 1,
    })
}

fn message_user(text: &str) -> Message {
    Message::User(UserMessage {
        content: StringOrBlocks::Text(text.into()),
        timestamp: 1,
    })
}

/// The oracle's `assistant()` fixture (`execution-assistant.test.ts:50-65`).
fn assistant_message(text: &str, stop_reason: StopReason) -> AssistantMessage {
    AssistantMessage {
        content: vec![AssistantBlock::Text(crate::ai::types::TextContent {
            text: text.into(),
            text_signature: None,
        })],
        api: "test".into(),
        provider: "provider".into(),
        model: "model".into(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        diagnostics: None,
        usage: usage(),
        stop_reason,
        deferred: None,
        error_message: (stop_reason == StopReason::Error).then(|| text.to_string()),
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 2,
    }
}

fn text_block(text: &str) -> AssistantBlock {
    AssistantBlock::Text(crate::ai::types::TextContent {
        text: text.into(),
        text_signature: None,
    })
}

fn message_text(block: &AssistantBlock) -> Option<String> {
    match block {
        AssistantBlock::Text(text) => Some(text.text.clone()),
        _ => None,
    }
}

/// The oracle's `toProviderMessages` (`execution-assistant.test.ts:67-72`).
fn to_provider_messages() -> Arc<ToProviderMessagesFn> {
    Arc::new(|messages, _context| {
        Box::pin(async move {
            messages
                .iter()
                .filter_map(|message| message.to_message())
                .collect()
        })
    })
}

/// Shared recorder for the observer lifecycle and the caller-side hooks, so
/// one flat order can be asserted (the oracle's `order` array).
#[derive(Clone, Default)]
struct Recorder {
    order: Arc<Mutex<Vec<String>>>,
    starts: Arc<Mutex<Vec<AssistantMessage>>>,
    updates: Arc<Mutex<Vec<AssistantMessage>>>,
    ends: Arc<Mutex<Vec<AssistantMessage>>>,
}

struct SharedObserver(Recorder);

impl AssistantStreamObserver for SharedObserver {
    fn start<'a>(
        &'a self,
        message: AssistantMessage,
        event: AssistantMessageEvent,
        _context: Context,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let mut order = self.0.order.lock().unwrap();
            assert_eq!(event.event_type(), "start");
            order.push("observer_start".into());
            drop(order);
            self.0.starts.lock().unwrap().push(message);
        })
    }

    fn update<'a>(
        &'a self,
        message: AssistantMessage,
        _event: AssistantMessageEvent,
        _context: Context,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.0.order.lock().unwrap().push("observer_update".into());
            self.0.updates.lock().unwrap().push(message);
        })
    }

    fn end<'a>(&'a self, message: SettledAssistantMessage, _context: Context) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.0.order.lock().unwrap().push("observer_end".into());
            self.0.ends.lock().unwrap().push(message);
        })
    }
}

struct LifecycleObserver(Arc<Mutex<Vec<String>>>);

impl AssistantStreamObserver for LifecycleObserver {
    fn start<'a>(
        &'a self,
        _m: AssistantMessage,
        _e: AssistantMessageEvent,
        _c: Context,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async {
            self.0.lock().unwrap().push("start".into());
        })
    }
    fn update<'a>(
        &'a self,
        _m: AssistantMessage,
        _e: AssistantMessageEvent,
        _c: Context,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async {
            self.0.lock().unwrap().push("update".into());
        })
    }
    fn end<'a>(&'a self, _m: SettledAssistantMessage, _c: Context) -> BoxFuture<'a, ()> {
        Box::pin(async {
            self.0.lock().unwrap().push("end".into());
        })
    }
}

/// Build a stream delivering the given events from a spawned task (the
/// oracle's `createAssistantMessageEventStream` + `queueMicrotask` pair).
fn manual_stream(events: Vec<AssistantMessageEvent>) -> AssistantMessageEventStream {
    let (sender, receiver) = tokio::sync::mpsc::channel(16);
    tokio::spawn(async move {
        for event in events {
            sender.send(event).await.expect("stream alive");
        }
    });
    receiver
}

/// Oracle "maps curated options and runs the assistant lifecycle without
/// mutating input" (`execution-assistant.test.ts:75-205`).
#[tokio::test]
async fn maps_curated_options_and_runs_the_assistant_lifecycle_without_mutating_input() {
    let input = vec![user("original")];
    let input_snapshot = input.clone();
    let request_model = model();
    let controller = CancellationToken::new();
    let recorder = Recorder::default();
    let converted_messages: Arc<Mutex<Vec<AgentMessage>>> = Arc::new(Mutex::new(Vec::new()));
    let request_payload: Arc<Mutex<Option<serde_json::Value>>> = Arc::new(Mutex::new(None));
    let received_context: Arc<Mutex<Option<AiContext>>> = Arc::new(Mutex::new(None));
    let received_options: Arc<Mutex<Option<AssistantRequestOptions>>> = Arc::new(Mutex::new(None));
    let response_metadata: Arc<Mutex<Option<AssistantResponseMetadata>>> =
        Arc::new(Mutex::new(None));

    let transform_order = {
        let order = Arc::clone(&recorder.order);
        move |event: &str| order.lock().unwrap().push(event.into())
    };
    let convert_order = {
        let order = Arc::clone(&recorder.order);
        move |event: &str| order.lock().unwrap().push(event.into())
    };
    let payload_order = {
        let order = Arc::clone(&recorder.order);
        move |event: &str| order.lock().unwrap().push(event.into())
    };
    let response_order = {
        let order = Arc::clone(&recorder.order);
        move |event: &str| order.lock().unwrap().push(event.into())
    };
    let request_order = {
        let order = Arc::clone(&recorder.order);
        move |event: &str| order.lock().unwrap().push(event.into())
    };
    let to_provider_writer = Arc::clone(&converted_messages);
    let before_payload_order = payload_order.clone();
    let before_payload_writer = Arc::clone(&request_payload);
    let response_metadata_writer = Arc::clone(&response_metadata);
    let request_context_writer = Arc::clone(&received_context);
    let request_options_writer = Arc::clone(&received_options);
    let request_model_for_request = request_model.clone();

    let config = HarnessAssistantStreamConfig {
        model: request_model.clone(),
        system_prompt: "system".into(),
        tools: None,
        thinking_level: ThinkingLevel::High,
        stream_options: AgentHarnessStreamOptions {
            transport: Some(Transport::Websocket),
            timeout_ms: Some(123),
            max_retries: Some(2),
            max_retry_delay_ms: Some(456),
            headers: Some(
                [("authorization".to_string(), "test".to_string())]
                    .into_iter()
                    .collect(),
            ),
            metadata: Some(
                [("tenant".to_string(), serde_json::json!("one"))]
                    .into_iter()
                    .collect(),
            ),
            cache_retention: Some(CacheRetention::Long),
            deferred: Some(DeferredFlag::Object {
                window: Some(DeferredWindow::OneHour),
            }),
        },
        transform_context: Some(Arc::new(move |mut context: HarnessRequestContext, _ctx| {
            transform_order("transform_context");
            context.messages.push(user("injected"));
            context.system_prompt = "transformed system".into();
            Box::pin(async move { Ok(context) })
        })),
        to_provider_messages: Arc::new(move |messages, _ctx| {
            convert_order("to_provider_messages");
            *to_provider_writer.lock().unwrap() = messages.clone();
            let result: Vec<Message> = messages
                .iter()
                .filter_map(|message| message.to_message())
                .collect();
            Box::pin(async move { result })
        }),
        before_payload: Some(Arc::new(move |payload, seen_model, _ctx| {
            before_payload_order("before_payload");
            assert_eq!(seen_model.id, "resolved");
            *before_payload_writer.lock().unwrap() = Some(payload.clone());
            Box::pin(async move { Ok(Some(serde_json::json!({ "replaced": true }))) })
        })),
        after_response: Some(Arc::new(move |message, metadata, _ctx| {
            response_order("after_response");
            *response_metadata_writer.lock().unwrap() = Some(metadata);
            let mut transformed = message.clone();
            transformed.content = vec![text_block("transformed")];
            Box::pin(async move { Ok(transformed) })
        })),
        request: Arc::new(move |context, options, _ctx| {
            request_order("request");
            *request_context_writer.lock().unwrap() = Some(context.clone());
            *request_options_writer.lock().unwrap() = Some(options.clone());
            let request_model = request_model_for_request.clone();
            Box::pin(async move {
                // `await options.onPayload?.(...)` — the harness callback runs
                // and its replacement value comes back.
                if let Some(on_payload) = &options.on_payload {
                    let replaced = (on_payload)(
                        serde_json::json!({ "original": true }),
                        {
                            let mut resolved = request_model.clone();
                            resolved.id = "resolved".into();
                            resolved
                        },
                        background_context(),
                    )
                    .await?;
                    assert_eq!(replaced, Some(serde_json::json!({ "replaced": true })));
                }
                // `await options.onResponse?.(...)` — metadata captured.
                if let Some(on_response) = &options.on_response {
                    on_response(AssistantResponseMetadata {
                        status: Some(201),
                        headers: Some(
                            [("request-id".to_string(), "r1".to_string())]
                                .into_iter()
                                .collect(),
                        ),
                    });
                }
                let initial = {
                    let mut message = assistant_message("", StopReason::Pending);
                    message.content = Vec::new();
                    message
                };
                let final_message = assistant_message("raw", StopReason::Stop);
                Ok(manual_stream(vec![
                    AssistantMessageEvent::Start { message: initial },
                    AssistantMessageEvent::TextStart { content_index: 0 },
                    AssistantMessageEvent::TextDelta {
                        content_index: 0,
                        delta: "raw".into(),
                    },
                    AssistantMessageEvent::Done {
                        reason: SuccessReason::Stop,
                        message: final_message,
                    },
                ]))
            })
        }),
        observer: Arc::new(SharedObserver(recorder.clone())),
    };

    let result = stream_harness_assistant(
        &input,
        &config,
        with_abort_signal(controller.clone(), background_context()),
    )
    .await
    .unwrap();

    assert_eq!(input, input_snapshot, "input list is not mutated");
    assert_eq!(
        *converted_messages.lock().unwrap(),
        vec![user("original"), user("injected")]
    );
    let context = received_context.lock().unwrap().clone().unwrap();
    assert_eq!(context.system_prompt.as_deref(), Some("transformed system"));
    assert_eq!(
        context.messages,
        vec![message_user("original"), message_user("injected")]
    );
    assert_eq!(
        *request_payload.lock().unwrap(),
        Some(serde_json::json!({ "original": true }))
    );
    let options = received_options.lock().unwrap().clone().unwrap();
    assert_eq!(options.simple.stream.transport, Some(Transport::Websocket));
    assert_eq!(options.simple.stream.timeout_ms, Some(123));
    assert_eq!(options.simple.stream.max_retries, Some(2));
    assert_eq!(options.simple.stream.max_retry_delay_ms, Some(456));
    assert_eq!(
        options.simple.stream.headers,
        Some(
            [("authorization".to_string(), Some("test".to_string()))]
                .into_iter()
                .collect()
        )
    );
    assert_eq!(
        options.simple.stream.metadata,
        Some(
            [("tenant".to_string(), serde_json::json!("one"))]
                .into_iter()
                .collect()
        )
    );
    assert_eq!(
        options.simple.stream.cache_retention,
        Some(CacheRetention::Long)
    );
    assert_eq!(
        options.simple.deferred,
        Some(DeferredFlag::Object {
            window: Some(DeferredWindow::OneHour)
        })
    );
    assert_eq!(options.simple.reasoning, Some(RequestThinkingLevel::High));
    // `signal: controller.signal` — the port forwards the context's signal
    // (linked to the controller's token); cancelling the controller cancels
    // it, so identity is asserted by behavior.
    let options_signal = options
        .simple
        .stream
        .signal
        .clone()
        .expect("signal forwarded");
    assert!(!options_signal.is_cancelled());
    controller.cancel();
    assert!(options_signal.is_cancelled());
    let metadata = response_metadata.lock().unwrap().clone().unwrap();
    assert_eq!(metadata.status, Some(201));
    assert_eq!(
        metadata.headers,
        Some(
            [("request-id".to_string(), "r1".to_string())]
                .into_iter()
                .collect()
        )
    );
    let starts = recorder.starts.lock().unwrap().clone();
    let updates = recorder.updates.lock().unwrap().clone();
    let ends = recorder.ends.lock().unwrap().clone();
    assert_eq!(starts.len(), 1);
    assert!(
        starts[0].content.is_empty(),
        "start snapshot is the initial message"
    );
    assert!(
        !updates.is_empty() && !updates[0].content.is_empty(),
        "update snapshot is distinct"
    );
    assert_eq!(ends.len(), 1);
    assert_eq!(ends[0], result);
    assert_eq!(
        result.content.first().and_then(message_text),
        Some("transformed".to_string())
    );
    // Port-protocol note: upstream's fixture pushes a single `text_delta`
    // (its events carry the live `partial`); the port's events reconstruct
    // the partial through the strict reducer, which requires `text_start`
    // before the delta — so two update events deliver the same text.
    assert_eq!(
        *recorder.order.lock().unwrap(),
        vec![
            "transform_context".to_string(),
            "to_provider_messages".to_string(),
            "request".to_string(),
            "before_payload".to_string(),
            "observer_start".to_string(),
            "observer_update".to_string(),
            "observer_update".to_string(),
            "after_response".to_string(),
            "observer_end".to_string(),
        ]
    );
}

/// Oracle "runs against the faux provider request boundary"
/// (`execution-assistant.test.ts:207-245`).
#[tokio::test]
async fn runs_against_the_faux_provider_request_boundary() {
    let faux = faux_provider(FauxProviderOptions {
        token_size: Some(FauxTokenSize {
            min: Some(1),
            max: Some(1),
        }),
        ..FauxProviderOptions::default()
    });
    faux.set_responses(vec![faux_assistant_message(
        "hello",
        FauxMessageOptions::default(),
    )
    .into()]);
    let lifecycle: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_context: Arc<Mutex<Vec<Message>>> = Arc::new(Mutex::new(Vec::new()));
    let lifecycle_writer = Arc::clone(&lifecycle);
    let seen_writer = Arc::clone(&seen_context);

    let mut models = create_models(CreateModelsOptions::default());
    models.set_provider(faux.provider.clone());
    let models = Arc::new(models);
    let request_model = faux.get_model(None).expect("faux default model");
    let stream_model = request_model.clone();

    let config = HarnessAssistantStreamConfig {
        model: request_model,
        system_prompt: "system".into(),
        tools: None,
        thinking_level: ThinkingLevel::Off,
        stream_options: AgentHarnessStreamOptions::default(),
        transform_context: None,
        to_provider_messages: to_provider_messages(),
        before_payload: None,
        after_response: None,
        request: Arc::new(move |context, options, _ctx| {
            *seen_writer.lock().unwrap() = context.messages.clone();
            let models = Arc::clone(&models);
            let stream_model = stream_model.clone();
            Box::pin(async move {
                Ok(models.stream_simple(
                    &stream_model,
                    &context,
                    Some(ModelsSimpleStreamOptions {
                        simple: options.simple,
                        transform_headers: None,
                    }),
                ))
            })
        }),
        observer: Arc::new(LifecycleObserver(lifecycle_writer)),
    };

    let result = stream_harness_assistant(&[user("prompt")], &config, background_context())
        .await
        .unwrap();

    assert_eq!(*seen_context.lock().unwrap(), vec![message_user("prompt")]);
    assert_eq!(
        result.content.iter().find_map(message_text),
        Some("hello".to_string())
    );
    let lifecycle = lifecycle.lock().unwrap().clone();
    assert_eq!(lifecycle[0], "start");
    assert_eq!(lifecycle.last(), Some(&"end".to_string()));
    assert!(
        lifecycle.iter().any(|event| event == "update"),
        "faux stream emits updates: {lifecycle:?}"
    );
}

/// Oracle "rejects a successful terminal event before start"
/// (`execution-assistant.test.ts:247-271`).
#[tokio::test]
async fn rejects_a_successful_terminal_event_before_start() {
    let final_message = assistant_message("complete", StopReason::Stop);
    let received_options: Arc<Mutex<Option<AssistantRequestOptions>>> = Arc::new(Mutex::new(None));
    let options_writer = Arc::clone(&received_options);
    let config = HarnessAssistantStreamConfig {
        model: model(),
        system_prompt: "system".into(),
        tools: None,
        thinking_level: ThinkingLevel::Off,
        stream_options: AgentHarnessStreamOptions::default(),
        transform_context: None,
        to_provider_messages: to_provider_messages(),
        before_payload: None,
        after_response: None,
        request: Arc::new(move |_context, options, _ctx| {
            *options_writer.lock().unwrap() = Some(options.clone());
            let final_message = final_message.clone();
            Box::pin(async move {
                Ok(manual_stream(vec![AssistantMessageEvent::Done {
                    reason: SuccessReason::Stop,
                    message: final_message,
                }]))
            })
        }),
        observer: Arc::new(LifecycleObserver(Arc::new(Mutex::new(Vec::new())))),
    };
    let error = stream_harness_assistant(&[user("prompt")], &config, background_context())
        .await
        .expect_err("done before start");
    assert!(error.to_string().contains("done before start"), "{error}");
    let options = received_options.lock().unwrap().clone().unwrap();
    assert_eq!(
        options.simple.reasoning, None,
        "off thinking sends no reasoning"
    );
}

/// Oracle "keeps the raw settlement when cancellation interrupts
/// after_response" (`execution-assistant.test.ts:273-308`).
#[tokio::test]
async fn keeps_the_raw_settlement_when_cancellation_interrupts_after_response() {
    let final_message = assistant_message("raw", StopReason::Stop);
    let request_final = final_message.clone();
    let ends: Arc<Mutex<Vec<AssistantMessage>>> = Arc::new(Mutex::new(Vec::new()));
    let ends_writer = Arc::clone(&ends);
    struct EndOnlyObserver(Arc<Mutex<Vec<AssistantMessage>>>);
    impl AssistantStreamObserver for EndOnlyObserver {
        fn start<'a>(
            &'a self,
            _m: AssistantMessage,
            _e: AssistantMessageEvent,
            _c: Context,
        ) -> BoxFuture<'a, ()> {
            Box::pin(async {})
        }
        fn update<'a>(
            &'a self,
            _m: AssistantMessage,
            _e: AssistantMessageEvent,
            _c: Context,
        ) -> BoxFuture<'a, ()> {
            Box::pin(async {})
        }
        fn end<'a>(&'a self, m: SettledAssistantMessage, _c: Context) -> BoxFuture<'a, ()> {
            let ends = Arc::clone(&self.0);
            Box::pin(async move {
                ends.lock().unwrap().push(m);
            })
        }
    }
    let initial = {
        let mut message = final_message.clone();
        message.content = Vec::new();
        message.stop_reason = StopReason::Pending;
        message
    };
    let config = HarnessAssistantStreamConfig {
        model: model(),
        system_prompt: "system".into(),
        tools: None,
        thinking_level: ThinkingLevel::Off,
        stream_options: AgentHarnessStreamOptions::default(),
        transform_context: None,
        to_provider_messages: to_provider_messages(),
        before_payload: None,
        after_response: Some(Arc::new(|_message, _metadata, _ctx| {
            Box::pin(async {
                let cancellation = CancellationToken::new();
                cancellation.cancel();
                Err(anyhow::Error::new(AbortRequested { cancellation }))
            })
        })),
        request: Arc::new(move |_context, _options, _ctx| {
            let initial = initial.clone();
            let final_message = request_final.clone();
            Box::pin(async move {
                Ok(manual_stream(vec![
                    AssistantMessageEvent::Start { message: initial },
                    AssistantMessageEvent::Done {
                        reason: SuccessReason::Stop,
                        message: final_message,
                    },
                ]))
            })
        }),
        observer: Arc::new(EndOnlyObserver(ends_writer)),
    };
    let result = stream_harness_assistant(&[user("prompt")], &config, background_context())
        .await
        .unwrap();
    assert_eq!(result, final_message);
    assert_eq!(*ends.lock().unwrap(), vec![final_message]);
}

/// Oracle "returns provider error settlements through the same lifecycle"
/// (`execution-assistant.test.ts:310-343`).
#[tokio::test]
async fn returns_provider_error_settlements_through_the_same_lifecycle() {
    let final_message = assistant_message("provider failed", StopReason::Error);
    let request_final = final_message.clone();
    let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let events_writer = Arc::clone(&events);
    struct ErrorLifecycleObserver {
        events: Arc<Mutex<Vec<String>>>,
    }
    impl AssistantStreamObserver for ErrorLifecycleObserver {
        fn start<'a>(
            &'a self,
            _m: AssistantMessage,
            _e: AssistantMessageEvent,
            _c: Context,
        ) -> BoxFuture<'a, ()> {
            // "pre-generation error must not synthesize start"
            panic!("start must not be called for an error-only stream");
        }
        fn update<'a>(
            &'a self,
            _m: AssistantMessage,
            _e: AssistantMessageEvent,
            _c: Context,
        ) -> BoxFuture<'a, ()> {
            let events = Arc::clone(&self.events);
            Box::pin(async move {
                events.lock().unwrap().push("update".into());
            })
        }
        fn end<'a>(&'a self, m: SettledAssistantMessage, _c: Context) -> BoxFuture<'a, ()> {
            let events = Arc::clone(&self.events);
            Box::pin(async move {
                events
                    .lock()
                    .unwrap()
                    .push(format!("end:{:?}", m.stop_reason));
            })
        }
    }
    let config = HarnessAssistantStreamConfig {
        model: model(),
        system_prompt: "system".into(),
        tools: None,
        thinking_level: ThinkingLevel::Off,
        stream_options: AgentHarnessStreamOptions::default(),
        transform_context: None,
        to_provider_messages: to_provider_messages(),
        before_payload: None,
        after_response: None,
        request: Arc::new(move |_context, _options, _ctx| {
            let final_message = request_final.clone();
            Box::pin(async move {
                Ok(manual_stream(vec![AssistantMessageEvent::Error {
                    reason: ErrorReason::Error,
                    error: final_message,
                }]))
            })
        }),
        observer: Arc::new(ErrorLifecycleObserver {
            events: events_writer,
        }),
    };
    let result = stream_harness_assistant(&[user("prompt")], &config, background_context())
        .await
        .unwrap();
    assert_eq!(result, final_message);
    assert_eq!(
        *events.lock().unwrap(),
        vec![format!("end:{:?}", StopReason::Error)]
    );
}
