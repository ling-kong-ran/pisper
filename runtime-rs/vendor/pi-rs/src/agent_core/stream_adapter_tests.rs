//! r14 stream boundary regression suite. The JSON oracle runs actual upstream
//! source; native concurrency and ModelRuntime tests supplement it (not SDK).
use super::*;
use crate::agent_core::agent_loop::{run_agent_loop, AgentLoopTurnUpdate};
use crate::ai::models::faux::FauxModelDefinition;
use crate::ai::models::{
    create_models, faux_assistant_message, faux_provider, CreateModelsOptions, FauxMessageOptions,
    FauxProviderOptions, FauxResponseStep, ModelsSimpleStreamOptions,
};
use crate::ai::transcript::{Context, TranscriptContext};
use crate::ai::types::events::{AssistantMessageEvent, ErrorReason, SuccessReason};
use crate::ai::types::options::SimpleStreamOptions;
use crate::ai::types::request_callbacks::ProviderResponse;
use crate::ai::types::{Model, ModelInput};
use serde_json::{json, Value};
use std::sync::atomic::AtomicUsize;
use std::time::Duration;
use tokio::sync::{mpsc, Notify, Semaphore};

type Trace = Arc<Mutex<Vec<Value>>>;
fn trace(log: &Trace, row: Value) {
    log.lock().unwrap().push(row);
}
fn model() -> Model {
    Model {
        r#type: None,
        prompt_cache: None,
        input_limits: None,
        id: "r14-model".into(),
        name: "r14-model".into(),
        api: "r14-api".into(),
        provider: "r14-provider".into(),
        reasoning: true,
        input: vec![ModelInput::Text],
        context_window: 4096,
        max_tokens: 1000,
        ..unknown_model()
    }
}
fn empty_models() -> Arc<Models> {
    Arc::new(create_models(CreateModelsOptions::default()))
}
fn user(text: &str) -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: StringOrBlocks::Text(text.into()),
        timestamp: 1,
    })
}
fn assistant(reason: StopReason, error: Option<&str>) -> AssistantMessage {
    let mut msg = faux_assistant_message(
        "ok",
        FauxMessageOptions {
            stop_reason: Some(reason),
            error_message: error.map(str::to_owned),
            timestamp: Some(2),
            ..Default::default()
        },
    );
    msg.api = model().api;
    msg.provider = model().provider;
    msg.model = model().id;
    msg
}
fn events(final_message: AssistantMessage, no_start: bool) -> Vec<AssistantMessageEvent> {
    let mut result = Vec::new();
    if !no_start {
        let mut start = final_message.clone();
        start.content.clear();
        start.stop_reason = StopReason::Pending;
        result.push(AssistantMessageEvent::Start { message: start });
    }
    result.push(match final_message.stop_reason {
        StopReason::Error | StopReason::Aborted => AssistantMessageEvent::Error {
            reason: if final_message.stop_reason == StopReason::Aborted {
                ErrorReason::Aborted
            } else {
                ErrorReason::Error
            },
            error: final_message,
        },
        _ => AssistantMessageEvent::Done {
            reason: SuccessReason::Stop,
            message: final_message,
        },
    });
    result
}
fn ready_stream(events: Vec<AssistantMessageEvent>) -> mpsc::Receiver<AssistantMessageEvent> {
    let (tx, rx) = mpsc::channel(events.len().max(1));
    for event in events {
        tx.try_send(event).unwrap();
    }
    rx
}
fn ready_adapter() -> Arc<StreamFn> {
    Arc::new(|_, _, _| {
        Box::pin(async {
            Ok(ready_stream(events(
                assistant(StopReason::Stop, None),
                false,
            )))
        })
    })
}
fn options(stream_fn: Arc<StreamFn>) -> AgentOptions {
    AgentOptions {
        initial_state: AgentInitialState {
            model: Some(model()),
            ..Default::default()
        },
        stream_fn: Some(stream_fn),
        ..Default::default()
    }
}
async fn bounded<T>(phase: &str, future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(8), future)
        .await
        .unwrap_or_else(|_| panic!("timeout: {phase}"))
}
fn event_summary(event: &AgentEvent) -> Value {
    let wire = serde_json::to_value(event).unwrap();
    let kind = wire["type"].as_str().unwrap();
    let mut row = json!({"type":kind});
    if wire.get("message").is_some() && kind != "turn_end" {
        row["role"] = wire["message"]["role"].clone();
    }
    if (kind == "message_end" && wire["message"]["role"] == "assistant") || kind == "turn_end" {
        row["reason"] = wire["message"]["stopReason"].clone();
        row["error"] = wire["message"]["errorMessage"].clone();
    }
    if kind == "agent_end" {
        row["count"] = json!(wire["messages"].as_array().unwrap().len());
    }
    row
}
fn sink(log: &Trace) -> AgentEventSink {
    let log = log.clone();
    Arc::new(move |event| {
        trace(&log, event_summary(&event));
        Box::pin(async {})
    })
}
fn record_stream(
    log: &Trace,
    model: &Model,
    context: &TranscriptContext,
    options: &SimpleStreamOptions,
) {
    let messages: Vec<Value> = context
        .messages()
        .iter()
        .map(|m| serde_json::to_value(m).unwrap())
        .collect();
    let texts: Vec<String> = messages
        .iter()
        .map(|m| match &m["content"] {
            Value::String(s) => s.clone(),
            Value::Array(blocks) => blocks
                .iter()
                .filter(|b| b["type"] == "text")
                .map(|b| b["text"].as_str().unwrap())
                .collect::<Vec<_>>()
                .join("\n"),
            _ => panic!("bad content"),
        })
        .collect();
    trace(
        log,
        json!({"phase":"stream","model":model.id,"roles":messages.iter().map(|m|m["role"].clone()).collect::<Vec<_>>(),
        "texts":texts,"normalized":true,"options":serde_json::to_value(options).unwrap(),
        "signal":options.stream.signal.is_some(),"payload":options.stream.callbacks.on_payload.is_some(),
        "response":options.stream.callbacks.on_response.is_some()}),
    );
}
fn install_callbacks(log: &Trace) -> RequestCallbacks {
    let p = log.clone();
    let r = log.clone();
    RequestCallbacks {
        on_provider_stream_event: None,
        on_payload: Some(Arc::new(move |payload, model| {
            trace(
                &p,
                json!({"phase":"payload","model":model.id,"payload":payload}),
            );
            Box::pin(async { Ok(Some(json!({"changed":true}))) })
        })),
        on_response: Some(Arc::new(move |response, model| {
            trace(
                &r,
                json!({"phase":"response","model":model.id,
            "response":{"status":response.status,"headers":response.headers}}),
            );
            Box::pin(async { Ok(()) })
        })),
    }
}
#[tokio::test]
async fn complete_upstream_source_oracle_matches_sixteen_stream_boundary_cases() {
    let oracle: Value = serde_json::from_str(include_str!("stream_adapter_oracle.json")).unwrap();
    assert_eq!(oracle["cases"].as_array().unwrap().len(), 16);
    for row in oracle["cases"].as_array().unwrap() {
        let spec = row["spec"].clone();
        let log: Trace = Default::default();
        let callbacks = if spec["callbacks"] == true {
            install_callbacks(&log)
        } else {
            RequestCallbacks::default()
        };
        let s = spec.clone();
        let stream_log = log.clone();
        let stream_fn: Arc<StreamFn> = Arc::new(move |model, context, options| {
            record_stream(&stream_log, &model, &context, &options);
            let spec = s.clone();
            let log = stream_log.clone();
            Box::pin(async move {
                if let Some(error) = spec["reject"].as_str() {
                    anyhow::bail!("{error}");
                }
                if spec["callbacks"] == true {
                    let result = options
                        .stream
                        .callbacks
                        .payload(json!({"original":true}), &model)
                        .await?;
                    trace(&log, json!({"phase":"payload_result","result":result}));
                    options
                        .stream
                        .callbacks
                        .response(
                            ProviderResponse {
                                status: 201,
                                headers: [("x-test".into(), "yes".into())].into(),
                            },
                            &model,
                        )
                        .await?;
                }
                let terminal = spec["terminal"].as_str();
                let reason = match terminal {
                    Some("error") => StopReason::Error,
                    Some("aborted") => StopReason::Aborted,
                    _ => StopReason::Stop,
                };
                Ok(ready_stream(events(
                    assistant(reason, terminal.map(|_| "terminal failure")),
                    spec["noStart"] == true,
                )))
            })
        });
        let get_api_key: Option<Arc<GetApiKeyFn>> =
            if spec.get("keys").is_some() || spec["keyReject"] == true {
                let s = spec.clone();
                let l = log.clone();
                let n = AtomicUsize::new(0);
                Some(Arc::new(move |provider| {
                    trace(&l, json!({"phase":"key","provider":provider}));
                    let reject = s["keyReject"] == true;
                    let key = s["keys"][n.fetch_add(1, Ordering::SeqCst)]
                        .as_str()
                        .map(str::to_owned);
                    Box::pin(async move {
                        if reject {
                            anyhow::bail!("key failed");
                        }
                        Ok(key)
                    })
                }))
            } else {
                None
            };
        let converted = spec["transform"] == true;
        let l = log.clone();
        let convert: Arc<ConvertToLlmFn> = Arc::new(move |messages| {
            if converted {
                trace(&l, json!({"phase":"convert"}));
            }
            Box::pin(async move {
                messages
                    .into_iter()
                    .map(|m| {
                        if m.role() == "custom" {
                            user("converted").to_message().unwrap()
                        } else {
                            m.to_message().unwrap()
                        }
                    })
                    .collect()
            })
        });
        let simple: SimpleStreamOptions =
            serde_json::from_value(spec.get("options").cloned().unwrap_or(json!({}))).unwrap();
        let mut error: Option<String> = None;
        let mut state = None;
        if spec["raw"] == true {
            let mut config = AgentLoopConfig::new(model(), convert);
            config.stream_fn = Some(stream_fn);
            config.get_api_key = get_api_key;
            config.stream_options = simple;
            if let Err(e) = bounded(
                "raw oracle",
                run_agent_loop(
                    vec![user("hello")],
                    AgentContext::default(),
                    config,
                    &empty_models(),
                    Some(CancellationToken::new()),
                    &sink(&log),
                ),
            )
            .await
            {
                error = Some(e.to_string());
            }
        } else {
            let mut opts = options(stream_fn);
            opts.initial_state.system_prompt = Some("system".into());
            opts.initial_state.thinking_level = spec["options"]
                .get("reasoning")
                .cloned()
                .map(|v| serde_json::from_value(v).unwrap());
            opts.get_api_key = get_api_key;
            opts.convert_to_llm = Some(convert);
            opts.callbacks = callbacks;
            opts.transport = simple.stream.transport;
            opts.session_id = simple.stream.session_id;
            opts.thinking_budgets = simple.thinking_budgets;
            opts.max_retry_delay_ms = simple.stream.max_retry_delay_ms;
            if converted {
                let l = log.clone();
                opts.transform_context = Some(Arc::new(move |mut messages| {
                    trace(&l, json!({"phase":"transform"}));
                    messages.push(
                        serde_json::from_value(json!({"role":"custom","content":"injected"}))
                            .unwrap(),
                    );
                    Box::pin(async move { messages })
                }));
            }
            let models = if spec["omit"] == true {
                // Native fallback is per-instance Models, not the JS global default.
                // Exercise that real fallback while projecting the same boundary trace.
                opts.stream_fn = None;
                let l = log.clone();
                let faux = faux_provider(FauxProviderOptions {
                    api: Some(model().api),
                    provider: Some(model().provider),
                    models: vec![FauxModelDefinition {
                        id: model().id,
                        ..Default::default()
                    }],
                    ..Default::default()
                });
                faux.set_responses(vec![FauxResponseStep::Factory(Arc::new(move |args| {
                    record_stream(
                        &l,
                        &args.model,
                        &args.context,
                        args.options.as_ref().unwrap(),
                    );
                    Box::pin(async {
                        let mut m = assistant(StopReason::Stop, None);
                        m.content.clear();
                        Ok(m)
                    })
                }))]);
                let mut models = create_models(CreateModelsOptions::default());
                models.set_provider(faux.provider.clone());
                Arc::new(models)
            } else {
                empty_models()
            };
            let agent = Agent::new(opts, models);
            let output = sink(&log);
            agent.subscribe(move |event, _| output(event));
            if spec["followup"] == true {
                agent.follow_up(user("follow"));
            }
            bounded("Agent oracle", agent.prompt(user("hello")))
                .await
                .unwrap();
            let current = agent.state();
            state = Some(
                json!({"streaming":current.is_streaming,"error":current.error_message,
                "roles":current.messages.iter().map(AgentMessage::role).collect::<Vec<_>>()}),
            );
        }
        assert_eq!(
            json!(*log.lock().unwrap()),
            row["trace"],
            "trace {}",
            spec["name"]
        );
        assert_eq!(json!(error), row["error"], "error {}", spec["name"]);
        if let Some(state) = state {
            assert_eq!(state, row["state"], "state {}", spec["name"]);
        }
    }
}

#[test]
fn options_debug_and_clones_preserve_callback_identity_without_exposing_keys() {
    let log = Trace::default();
    let cb = install_callbacks(&log);
    let mut opts = options(ready_adapter());
    opts.callbacks = cb.clone();
    opts.get_api_key = Some(Arc::new(|_| {
        Box::pin(async { Ok(Some("private-captured-value".into())) })
    }));
    let cloned = opts.clone();
    assert_eq!(cloned.callbacks, cb);
    assert!(Arc::ptr_eq(
        cloned.stream_fn.as_ref().unwrap(),
        opts.stream_fn.as_ref().unwrap()
    ));
    let debug = format!("{opts:?}");
    assert!(debug.contains("stream_fn: true"));
    assert!(!debug.contains("private-captured-value"));
    let agent = Agent::new(cloned, empty_models());
    let runtime = agent.runtime();
    assert_eq!(runtime.transport, Transport::Auto);
    assert_eq!(runtime.callbacks, cb);
    assert!(!format!("{runtime:?}").contains("private-captured-value"));
}

#[tokio::test]
async fn runtime_adapter_callbacks_and_transport_can_change_between_runs() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let log = observed.clone();
    let original: Arc<StreamFn> = Arc::new(move |_, _, options| {
        log.lock().unwrap().push(("first", options));
        Box::pin(async {
            Ok(ready_stream(events(
                assistant(StopReason::Stop, None),
                false,
            )))
        })
    });
    let agent = Agent::new(options(original), empty_models());
    agent.prompt("one").await.unwrap();
    let callbacks = install_callbacks(&Trace::default());
    let log = observed.clone();
    {
        let mut runtime = agent.runtime();
        runtime.transport = Transport::WebsocketCached;
        runtime.callbacks = callbacks.clone();
        runtime.stream_fn = Some(Arc::new(move |_, _, options| {
            log.lock().unwrap().push(("second", options));
            Box::pin(async {
                Ok(ready_stream(events(
                    assistant(StopReason::Stop, None),
                    false,
                )))
            })
        }));
    }
    agent.prompt("two").await.unwrap();
    let records = observed.lock().unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].0, "first");
    assert_eq!(records[0].1.stream.transport, Some(Transport::Auto));
    assert_eq!(records[1].0, "second");
    assert_eq!(
        records[1].1.stream.transport,
        Some(Transport::WebsocketCached)
    );
    assert_eq!(records[1].1.stream.callbacks, callbacks);
}
#[tokio::test]
async fn pending_factory_is_awaited_and_receives_the_live_abort_token() {
    let entered = Arc::new(Notify::new());
    let gate = Arc::new(Semaphore::new(0));
    let seen = Arc::new(Mutex::new(None));
    let e = entered.clone();
    let g = gate.clone();
    let s = seen.clone();
    let factory: Arc<StreamFn> = Arc::new(move |_, _, options| {
        let e = e.clone();
        let g = g.clone();
        let s = s.clone();
        Box::pin(async move {
            let token = options.stream.signal.unwrap();
            *s.lock().unwrap() = Some(token.clone());
            e.notify_one();
            g.acquire().await.unwrap().forget();
            assert!(token.is_cancelled());
            Ok(ready_stream(events(
                assistant(StopReason::Aborted, Some("cooperative cancel")),
                false,
            )))
        })
    });
    let agent = Arc::new(Agent::new(options(factory), empty_models()));
    let task_agent = agent.clone();
    let task = tokio::spawn(async move { task_agent.prompt("hello").await });
    bounded("factory entered", entered.notified()).await;
    assert!(agent.state().is_streaming);
    assert!(!task.is_finished());
    agent.abort();
    assert!(seen.lock().unwrap().as_ref().unwrap().is_cancelled());
    assert!(!task.is_finished());
    let idle = agent.wait_for_idle();
    tokio::pin!(idle);
    assert!(futures::poll!(&mut idle).is_pending());
    gate.add_permits(1);
    bounded("factory cancel settlement", task)
        .await
        .unwrap()
        .unwrap();
    bounded("idle", idle).await;
    assert_eq!(
        agent.state().error_message.as_deref(),
        Some("cooperative cancel")
    );
    assert!(!agent.state().is_streaming);
}

#[tokio::test]
async fn aborted_factory_rejection_uses_one_final_aborted_lifecycle() {
    let entered = Arc::new(Notify::new());
    let gate = Arc::new(Semaphore::new(0));
    let e = entered.clone();
    let g = gate.clone();
    let factory: Arc<StreamFn> = Arc::new(move |_, _, options| {
        let e = e.clone();
        let g = g.clone();
        Box::pin(async move {
            e.notify_one();
            g.acquire().await.unwrap().forget();
            assert!(options.stream.signal.unwrap().is_cancelled());
            anyhow::bail!("late rejection")
        })
    });
    let agent = Arc::new(Agent::new(options(factory), empty_models()));
    let log = Trace::default();
    let output = sink(&log);
    agent.subscribe(move |event, _| output(event));
    let a = agent.clone();
    let task = tokio::spawn(async move { a.prompt("hello").await });
    bounded("reject factory entered", entered.notified()).await;
    agent.abort();
    assert!(!task.is_finished());
    gate.add_permits(1);
    bounded("reject factory settled", task)
        .await
        .unwrap()
        .unwrap();
    let rows = log.lock().unwrap();
    assert_eq!(rows.iter().filter(|r| r["type"] == "agent_end").count(), 1);
    assert_eq!(rows.iter().filter(|r| r["type"] == "turn_end").count(), 1);
    assert!(rows.iter().any(|r| r["type"] == "message_end"
        && r["reason"] == "aborted"
        && r["error"] == "late rejection"));
    assert!(agent.signal().is_none());
    assert!(!agent.state().is_streaming);
}

#[tokio::test]
async fn pending_key_is_not_skipped_or_raced_on_abort_and_factory_still_sees_abort() {
    let entered = Arc::new(Notify::new());
    let gate = Arc::new(Semaphore::new(0));
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let factory: Arc<StreamFn> = Arc::new(move |_, _, options| {
        count.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            assert_eq!(options.stream.api_key.as_deref(), Some("late-offline-key"));
            assert!(options.stream.signal.unwrap().is_cancelled());
            Ok(ready_stream(events(
                assistant(StopReason::Aborted, Some("cancelled after key")),
                true,
            )))
        })
    });
    let mut opts = options(factory);
    let e = entered.clone();
    let g = gate.clone();
    opts.get_api_key = Some(Arc::new(move |_| {
        let e = e.clone();
        let g = g.clone();
        Box::pin(async move {
            e.notify_one();
            g.acquire().await.unwrap().forget();
            Ok(Some("late-offline-key".into()))
        })
    }));
    let agent = Arc::new(Agent::new(opts, empty_models()));
    let a = agent.clone();
    let task = tokio::spawn(async move { a.prompt("hello").await });
    bounded("key entered", entered.notified()).await;
    agent.abort();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(!task.is_finished());
    gate.add_permits(1);
    bounded("key settlement", task).await.unwrap().unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn adapter_stream_obeys_awaited_listener_and_bounded_channel_backpressure() {
    let (tx, rx) = mpsc::channel(1);
    tx.send(events(assistant(StopReason::Stop, None), false).remove(0))
        .await
        .unwrap();
    let receiver = Arc::new(Mutex::new(Some(rx)));
    let factory: Arc<StreamFn> = Arc::new(move |_, _, _| {
        let rx = receiver.lock().unwrap().take().unwrap();
        Box::pin(async move { Ok(rx) })
    });
    let entered = Arc::new(Notify::new());
    let buffered = Arc::new(Notify::new());
    let gate = Arc::new(Semaphore::new(0));
    let agent = Arc::new(Agent::new(options(factory), empty_models()));
    let log = Trace::default();
    let l = log.clone();
    let e = entered.clone();
    let g = gate.clone();
    agent.subscribe(move |event, _| {
        trace(&l, event_summary(&event));
        let e = e.clone();
        let g = g.clone();
        Box::pin(async move {
            if matches!(
                event,
                AgentEvent::MessageStart {
                    message: AgentMessage::Assistant(_)
                }
            ) {
                e.notify_one();
                g.acquire().await.unwrap().forget();
            }
        })
    });
    let sent = Arc::new(AtomicUsize::new(0));
    let n = sent.clone();
    let b = buffered.clone();
    let sender = tx.clone();
    let producer = tokio::spawn(async move {
        sender
            .send(AssistantMessageEvent::TextStart { content_index: 0 })
            .await
            .unwrap();
        n.fetch_add(1, Ordering::SeqCst);
        b.notify_one();
        sender
            .send(AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "ok".into(),
            })
            .await
            .unwrap();
        n.fetch_add(1, Ordering::SeqCst);
        sender
            .send(AssistantMessageEvent::TextEnd {
                content_index: 0,
                content: "ok".into(),
            })
            .await
            .unwrap();
        n.fetch_add(1, Ordering::SeqCst);
        sender
            .send(AssistantMessageEvent::Done {
                reason: SuccessReason::Stop,
                message: assistant(StopReason::Stop, None),
            })
            .await
            .unwrap();
    });
    let a = agent.clone();
    let task = tokio::spawn(async move { a.prompt("hello").await });
    bounded("listener entered", entered.notified()).await;
    bounded("buffer filled", buffered.notified()).await;
    assert_eq!(sent.load(Ordering::SeqCst), 1);
    assert_eq!(tx.capacity(), 0);
    assert!(!producer.is_finished());
    assert!(!task.is_finished());
    assert!(!log
        .lock()
        .unwrap()
        .iter()
        .any(|r| r["type"] == "message_update"));
    gate.add_permits(1);
    bounded("backpressure run", task).await.unwrap().unwrap();
    bounded("producer", producer).await.unwrap();
    assert_eq!(sent.load(Ordering::SeqCst), 3);
    assert_eq!(
        log.lock()
            .unwrap()
            .iter()
            .filter(|r| r["type"] == "message_update")
            .count(),
        3
    );
    assert!(tx.is_closed());
}

#[tokio::test]
async fn custom_stream_agent_end_listener_remains_part_of_idle_settlement() {
    let entered = Arc::new(Notify::new());
    let gate = Arc::new(Semaphore::new(0));
    let e = entered.clone();
    let g = gate.clone();
    let agent = Arc::new(Agent::new(options(ready_adapter()), empty_models()));
    agent.subscribe(move |event, _| {
        let e = e.clone();
        let g = g.clone();
        Box::pin(async move {
            if matches!(event, AgentEvent::AgentEnd { .. }) {
                e.notify_one();
                g.acquire().await.unwrap().forget();
            }
        })
    });
    let a = agent.clone();
    let task = tokio::spawn(async move { a.prompt("hello").await });
    bounded("agent_end entered", entered.notified()).await;
    assert!(agent.state().is_streaming);
    assert!(agent.signal().is_some());
    assert!(!task.is_finished());
    gate.add_permits(1);
    bounded("agent_end settled", task).await.unwrap().unwrap();
    assert!(!agent.state().is_streaming);
}

#[tokio::test]
async fn key_uses_updated_model_each_turn_and_off_clears_inherited_reasoning() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let stream_log = observed.clone();
    let keys = Arc::new(Mutex::new(Vec::new()));
    let k = keys.clone();
    let mut config = AgentLoopConfig::new(model(), Arc::new(default_convert_to_llm));
    config.stream_options.reasoning = Some(crate::ai::types::primitives::ThinkingLevel::High);
    config.stream_fn = Some(Arc::new(move |model, _, options| {
        stream_log.lock().unwrap().push((model, options));
        Box::pin(async {
            Ok(ready_stream(events(
                assistant(StopReason::Stop, None),
                false,
            )))
        })
    }));
    config.get_api_key = Some(Arc::new(move |provider| {
        k.lock().unwrap().push(provider.clone());
        Box::pin(async move { Ok(Some(provider)) })
    }));
    let queued = AtomicBool::new(false);
    config.get_follow_up_messages = Some(Arc::new(move || {
        let first = !queued.swap(true, Ordering::SeqCst);
        Box::pin(async move {
            if first {
                vec![user("follow")]
            } else {
                vec![]
            }
        })
    }));
    config.prepare_next_turn = Some(Arc::new(|_| {
        Box::pin(async {
            let mut next = model();
            next.id = "other".into();
            next.provider = "other-provider".into();
            Some(AgentLoopTurnUpdate {
                model: Some(next),
                thinking_level: Some(ThinkingLevel::Off),
                ..Default::default()
            })
        })
    }));
    bounded(
        "updated model",
        run_agent_loop(
            vec![user("hello")],
            AgentContext::default(),
            config,
            &empty_models(),
            None,
            &sink(&Trace::default()),
        ),
    )
    .await
    .unwrap();
    let seen = observed.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert!(seen[0].1.reasoning.is_some());
    assert_eq!(seen[1].1.reasoning, None);
    assert_eq!(seen[1].0.id, "other");
    assert_eq!(
        *keys.lock().unwrap(),
        vec!["r14-provider", "other-provider"]
    );
    assert_eq!(seen[1].1.stream.api_key.as_deref(), Some("other-provider"));
}

#[tokio::test]
async fn loop_run_signal_overrides_stored_signal_even_when_absent() {
    for run_signal in [None, Some(CancellationToken::new())] {
        let mut config = AgentLoopConfig::new(model(), Arc::new(default_convert_to_llm));
        let stored = CancellationToken::new();
        stored.cancel();
        config.stream_options.stream.signal = Some(stored);
        let present = run_signal.is_some();
        config.stream_fn = Some(Arc::new(move |_, _, options| {
            assert_eq!(options.stream.signal.is_some(), present);
            assert!(options
                .stream
                .signal
                .as_ref()
                .is_none_or(|t| !t.is_cancelled()));
            Box::pin(async {
                Ok(ready_stream(events(
                    assistant(StopReason::Stop, None),
                    false,
                )))
            })
        }));
        run_agent_loop(
            vec![user("hello")],
            AgentContext::default(),
            config,
            &empty_models(),
            run_signal,
            &sink(&Trace::default()),
        )
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn closed_custom_stream_retains_defensive_non_hanging_error_terminal() {
    for with_start in [false, true] {
        let factory: Arc<StreamFn> = Arc::new(move |_, _, _| {
            Box::pin(async move {
                Ok(ready_stream(if with_start {
                    vec![events(assistant(StopReason::Stop, None), false).remove(0)]
                } else {
                    vec![]
                }))
            })
        });
        let agent = Agent::new(options(factory), empty_models());
        let log = Trace::default();
        let output = sink(&log);
        agent.subscribe(move |event, _| output(event));
        bounded("missing terminal", agent.prompt("hello"))
            .await
            .unwrap();
        assert_eq!(
            agent.state().error_message.as_deref(),
            Some("Assistant stream ended without a terminal event")
        );
        assert_eq!(
            log.lock()
                .unwrap()
                .iter()
                .filter(|r| r["type"] == "agent_end")
                .count(),
            1
        );
    }
}
// Native integration through the real ModelRuntime and HTTP provider, with only
// a loopback wiremock endpoint. This is adapter integration, not an SDK factory.
async fn local_runtime(
    base_url: &str,
) -> (
    crate::coding_agent::core::model_runtime::ModelRuntime,
    Model,
) {
    use crate::ai::auth::credential_store::InMemoryCredentialStore;
    use crate::ai::auth::types::{
        ApiKeyAuth, ApiKeyAuthInput, AuthError, AuthResult, ModelAuth, ProviderAuth,
    };
    use crate::ai::models::provider::{create_provider, ApiImpls, CreateProviderOptions};
    use crate::coding_agent::core::model_runtime::{CreateModelRuntimeOptions, ModelRuntime};
    use crate::coding_agent::core::models_store::InMemoryCodingAgentModelsStore;
    struct OfflineAuth;
    impl ApiKeyAuth for OfflineAuth {
        fn name(&self) -> &str {
            "r14 offline test"
        }
        fn resolve<'a>(
            &'a self,
            _: ApiKeyAuthInput<'a>,
        ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
            Box::pin(async {
                Ok(Some(AuthResult {
                    auth: ModelAuth {
                        api_key: Some("offline-test-key-not-a-secret".into()),
                        ..Default::default()
                    },
                    ..Default::default()
                }))
            })
        }
    }
    let runtime = ModelRuntime::create(CreateModelRuntimeOptions {
        credentials: Some(Arc::new(InMemoryCredentialStore::default())),
        models_path: Some(None),
        models_store: Some(Arc::new(InMemoryCodingAgentModelsStore::default())),
        allow_model_network: false,
        refresh_on_create: Some(false),
        ..Default::default()
    })
    .await
    .unwrap();
    let mut model = model();
    model.api = "openai-completions".into();
    model.base_url = format!("{base_url}/v1");
    model.reasoning = false;
    model.headers = Some([("x-model".into(), Some("model-header".into()))].into());
    runtime
        .register_native_provider(create_provider(CreateProviderOptions {
            filter_all_models: None,
            images: crate::ai::models::provider::ImagesImpls::new(),
            classifiers: crate::ai::models::provider::ClassifiersImpls::new(),
            id: model.provider.clone(),
            name: None,
            base_url: None,
            headers: None,
            auth: ProviderAuth {
                api_key: Some(Arc::new(OfflineAuth)),
                oauth: None,
            },
            models: vec![crate::ai::types::AnyModel::Chat(model.clone())],
            fetch_models: None,
            filter_models: None,
            api: ApiImpls::Single(Arc::new(
                crate::ai::api::openai_completions::OpenAiCompletions,
            )),
        }))
        .await
        .unwrap();
    (runtime, model)
}
fn runtime_adapter(
    runtime: crate::coding_agent::core::model_runtime::ModelRuntime,
    phases: Arc<Mutex<Vec<&'static str>>>,
) -> Arc<StreamFn> {
    Arc::new(move |model, transcript, mut options| {
        let runtime = runtime.clone();
        let phases = phases.clone();
        Box::pin(async move {
            options.stream.max_retries = Some(0);
            options.stream.headers =
                Some([("x-request".into(), Some("request-header".into()))].into());
            let context = Context {
                messages: transcript.messages().to_vec(),
                ..Default::default()
            };
            Ok(runtime.stream_simple(
                &model,
                &context,
                Some(ModelsSimpleStreamOptions {
                    simple: options,
                    transform_headers: Some(Arc::new(move |mut headers| {
                        let phases = phases.clone();
                        Box::pin(async move {
                            phases.lock().unwrap().push("headers");
                            assert_eq!(headers.get("x-model"), Some(&Some("model-header".into())));
                            assert_eq!(
                                headers.get("x-request"),
                                Some(&Some("request-header".into()))
                            );
                            headers.insert("x-adapter".into(), Some("after-auth".into()));
                            headers
                        })
                    })),
                }),
            ))
        })
    })
}
async fn mount_response(server: &wiremock::MockServer) {
    let body=concat!("data: {\"id\":\"r14\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"r14\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n","data: [DONE]\n\n");
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/chat/completions"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .insert_header("x-r14-response", "received")
                .set_body_string(body),
        )
        .mount(server)
        .await;
}

#[tokio::test]
async fn model_runtime_adapter_forwards_headers_payload_response_and_awaits_payload() {
    let server = wiremock::MockServer::start().await;
    mount_response(&server).await;
    let (runtime, model) = local_runtime(&server.uri()).await;
    let phases = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(Notify::new());
    let gate = Arc::new(Semaphore::new(0));
    let e = entered.clone();
    let g = gate.clone();
    let p = phases.clone();
    let r = phases.clone();
    let mut opts = options(runtime_adapter(runtime, phases.clone()));
    opts.initial_state.model = Some(model);
    opts.callbacks = RequestCallbacks {
        on_provider_stream_event: None,
        on_payload: Some(Arc::new(move |mut payload, _| {
            let p = p.clone();
            let e = e.clone();
            let g = g.clone();
            Box::pin(async move {
                p.lock().unwrap().push("payload");
                e.notify_one();
                g.acquire().await.unwrap().forget();
                payload["r14_probe"] = json!("changed");
                Ok(Some(payload))
            })
        })),
        on_response: Some(Arc::new(move |response, _| {
            r.lock().unwrap().push("response");
            assert_eq!(response.status, 200);
            assert_eq!(
                response.headers.get("x-r14-response").map(String::as_str),
                Some("received")
            );
            Box::pin(async { Ok(()) })
        })),
    };
    let agent = Arc::new(Agent::new(opts, empty_models()));
    let a = agent.clone();
    let task = tokio::spawn(async move { a.prompt("hello").await });
    bounded("payload entered", entered.notified()).await;
    assert!(server.received_requests().await.unwrap().is_empty());
    assert!(!task.is_finished());
    gate.add_permits(1);
    bounded("runtime adapter request", task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        *phases.lock().unwrap(),
        vec!["headers", "payload", "response"]
    );
    assert_eq!(agent.state().error_message, None);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["r14_probe"], "changed");
    assert_eq!(requests[0].headers.get("x-adapter").unwrap(), "after-auth");
    assert_eq!(
        requests[0].headers.get("authorization").unwrap(),
        "Bearer offline-test-key-not-a-secret"
    );
}

#[tokio::test]
async fn model_runtime_callback_failures_settle_as_terminal_events_not_factory_rejects() {
    for fail_payload in [true, false] {
        let server = wiremock::MockServer::start().await;
        mount_response(&server).await;
        let (runtime, model) = local_runtime(&server.uri()).await;
        let mut opts = options(runtime_adapter(runtime, Default::default()));
        opts.initial_state.model = Some(model);
        opts.callbacks = RequestCallbacks {
            on_provider_stream_event: None,
            on_payload: Some(Arc::new(move |_, _| {
                Box::pin(async move {
                    if fail_payload {
                        anyhow::bail!("payload hook failed")
                    }
                    Ok(None)
                })
            })),
            on_response: Some(Arc::new(|_, _| {
                Box::pin(async { anyhow::bail!("response hook failed") })
            })),
        };
        let agent = Agent::new(opts, empty_models());
        let log = Trace::default();
        let output = sink(&log);
        agent.subscribe(move |event, _| output(event));
        bounded("callback failure", agent.prompt("hello"))
            .await
            .unwrap();
        assert!(agent
            .state()
            .error_message
            .as_ref()
            .unwrap()
            .contains(if fail_payload {
                "payload hook failed"
            } else {
                "response hook failed"
            }));
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            usize::from(!fail_payload)
        );
        let rows = log.lock().unwrap();
        assert_eq!(rows.iter().filter(|r| r["type"] == "agent_end").count(), 1);
        assert_eq!(
            rows.iter()
                .filter(|r| r["type"] == "message_end" && r["role"] == "assistant")
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn adapter_factory_and_key_callbacks_can_reenter_agent_without_a_held_mutex() {
    let slot: Arc<Mutex<std::sync::Weak<Agent>>> = Default::default();
    let stream_slot = slot.clone();
    let key_slot = slot.clone();
    let factory: Arc<StreamFn> = Arc::new(move |_, _, options| {
        let agent = stream_slot.lock().unwrap().upgrade().unwrap();
        assert!(agent.state().is_streaming);
        assert_eq!(agent.runtime().transport, Transport::Auto);
        assert_eq!(options.stream.api_key.as_deref(), Some("key"));
        agent.runtime().session_id = Some("changed-by-stream".into());
        Box::pin(async {
            Ok(ready_stream(events(
                assistant(StopReason::Stop, None),
                false,
            )))
        })
    });
    let mut opts = options(factory);
    opts.get_api_key = Some(Arc::new(move |_| {
        let agent = key_slot.lock().unwrap().upgrade().unwrap();
        agent.runtime().max_retry_delay_ms = Some(7);
        assert!(agent.state().is_streaming);
        Box::pin(async { Ok(Some("key".into())) })
    }));
    let agent = Arc::new(Agent::new(opts, empty_models()));
    *slot.lock().unwrap() = Arc::downgrade(&agent);
    bounded("reentrant callbacks", agent.prompt("hello"))
        .await
        .unwrap();
    assert_eq!(
        agent.runtime().session_id.as_deref(),
        Some("changed-by-stream")
    );
}
