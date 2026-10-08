//! Offline real HTTP tests for the process-local Anthropic request callbacks.
use super::*;
use crate::ai::transcript::{normalize_context, Context};
use crate::ai::types::request_callbacks::RequestCallbacks;
use crate::ai::types::CacheRetention;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;
use wiremock::{Mock, MockServer, ResponseTemplate};

const END: &str = "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

fn model() -> Model {
    Model {
        r#type: None,
        prompt_cache: None,
        input_limits: None,
        id: "claude-callback".into(),
        name: "Callback fixture".into(),
        api: API.into(),
        provider: "anthropic".into(),
        base_url: "https://api.anthropic.com".into(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![crate::ai::types::ModelInput::Text],
        cost: ModelCost::default(),
        context_window: 200000,
        max_tokens: 64,
        sampling_params: None,
        sampling_params_by_thinking_level: None,
        headers: None,
        compat: None,
    }
}
fn context() -> TranscriptContext {
    normalize_context(&Context {
        system_prompt: None,
        messages: vec![],
        tools: None,
    })
}
fn config(server: &MockServer) -> ProviderConfig {
    ProviderConfig {
        base_url: server.uri(),
        api_key: "local-test-only".into(),
        max_tokens: 64,
    }
}
fn success(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .set_body_raw(body.as_bytes(), "text/event-stream")
        .insert_header("x-callback", "observed")
}
async fn mount(server: &MockServer, body: &str) {
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/messages"))
        .and(wiremock::matchers::query_param("beta", "true"))
        .respond_with(success(body))
        .mount(server)
        .await;
}
async fn collect(mut rx: mpsc::Receiver<AssistantMessageEvent>) -> Vec<AssistantMessageEvent> {
    tokio::time::timeout(Duration::from_secs(5), async move {
        let mut events = vec![];
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    })
    .await
    .expect("bounded local stream")
}
fn terminal(events: &[AssistantMessageEvent]) -> &AssistantMessage {
    match events.last().expect("terminal event") {
        AssistantMessageEvent::Done { message, .. } => message,
        AssistantMessageEvent::Error { error, .. } => error,
        other => panic!("non-terminal {other:?}"),
    }
}
fn types(events: &[AssistantMessageEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(AssistantMessageEvent::event_type)
        .collect()
}

async fn compare_oracle(simple: bool) {
    let fixture: Value = serde_json::from_str(include_str!("callback_oracle.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let input = &case["input"];
        let server = MockServer::start().await;
        let trace = Arc::new(Mutex::new(Vec::<String>::new()));
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
        let metadata = Arc::new(Mutex::new(Vec::<Value>::new()));
        let request_trace = Arc::clone(&trace);
        let request_log = Arc::clone(&requests);
        let statuses: Vec<u16> = input
            .get("statuses")
            .map(|v| {
                v.as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_u64().unwrap() as u16)
                    .collect()
            })
            .unwrap_or(vec![200]);
        let sse_error = input["sseError"] == true;
        Mock::given(wiremock::matchers::method("POST"))
            .respond_with(move |request: &wiremock::Request| {
                request_trace.lock().unwrap().push("request".into());
                let headers: BTreeMap<_,_> = ["anthropic-beta", "anthropic-user-profile-id", "anthropic-workspace-id"]
                    .into_iter().filter_map(|key| request.headers.get(key).map(|value| (key, value.to_str().unwrap()))).collect();
                let mut log = request_log.lock().unwrap();
                let status = statuses.get(log.len()).copied().unwrap_or(200);
                log.push(json!({"url":format!("{}?{}",request.url.path(),request.url.query().unwrap_or_default()),"body":serde_json::from_slice::<Value>(&request.body).unwrap(),"headers":headers}));
                if status==200 { success(if sse_error { "event: error\ndata: stream refused\n\n" } else { END }) }
                else { ResponseTemplate::new(status).insert_header("retry-after-ms","0").set_body_string("denied") }
            }).mount(&server).await;
        let signal = CancellationToken::new();
        let mut options = StreamOptions {
            signal: Some(signal.clone()),
            max_retries: Some(2),
            cache_retention: Some(CacheRetention::None),
            ..Default::default()
        };
        if let Some(beta) = input.get("clientBeta").and_then(Value::as_str) {
            options.headers = Some(BTreeMap::from([(
                "anthropic-beta".into(),
                Some(beta.into()),
            )]));
        }
        if input["cancel"] == "before" {
            signal.cancel();
        }
        if input["payload"] != "absent" {
            let trace = Arc::clone(&trace);
            let seen = Arc::clone(&seen);
            let cancel = input["cancel"] == "payload";
            let fail = input["payload"] == "error";
            let replacement = input.get("replacement").cloned();
            let signal = signal.clone();
            options.callbacks.on_payload = Some(Arc::new(move |payload, model| {
                assert_eq!(model.id, "claude-callback");
                trace.lock().unwrap().push("payload".into());
                seen.lock().unwrap().push(payload);
                if cancel {
                    signal.cancel();
                }
                let replacement = replacement.clone();
                Box::pin(async move {
                    if fail {
                        anyhow::bail!("payload refused");
                    }
                    Ok(replacement)
                })
            }));
        }
        if input["response"] != false {
            let trace = Arc::clone(&trace);
            let metadata = Arc::clone(&metadata);
            let cancel = input["cancel"] == "response";
            let fail = input["response"] == "error";
            let signal = signal.clone();
            options.callbacks.on_response = Some(Arc::new(move |response, model| {
                assert_eq!(model.id, "claude-callback");
                trace.lock().unwrap().push("response".into());
                metadata.lock().unwrap().push(json!({"status":response.status,"headers":{"content-type":response.headers.get("content-type"),"x-callback":response.headers.get("x-callback")}}));
                if cancel {
                    signal.cancel();
                }
                Box::pin(async move {
                    if fail {
                        anyhow::bail!("response refused");
                    }
                    Ok(())
                })
            }));
        }
        let api = AnthropicMessages;
        let mut rx = if simple {
            api.stream_simple(
                &config(&server),
                &model(),
                &context(),
                &SimpleStreamOptions {
                    stream: options,
                    ..Default::default()
                },
            )
        } else {
            api.stream(&config(&server), &model(), &context(), &options)
        };
        let mut events = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(event) = rx.recv().await {
                trace.lock().unwrap().push(event.event_type().into());
                events.push(event);
            }
        })
        .await
        .expect("bounded oracle case");
        let last = terminal(&events);
        let actual = json!({"trace":*trace.lock().unwrap(),"seen":*seen.lock().unwrap(),"requests":*requests.lock().unwrap(),"metadata":*metadata.lock().unwrap(),"stopReason":last.stop_reason,"error":last.error_message});
        assert_eq!(actual, case["output"], "{} simple={simple}", input["name"]);
    }
}
#[tokio::test]
async fn callbacks_normal_stream_matches_actual_upstream_lifecycle_oracle() {
    compare_oracle(false).await;
}
#[tokio::test]
async fn callbacks_simple_stream_matches_actual_upstream_lifecycle_oracle() {
    compare_oracle(true).await;
}

#[tokio::test]
async fn callbacks_payload_is_awaited_before_send_even_when_cancelled() {
    let server = MockServer::start().await;
    mount(&server, END).await;
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let signal = CancellationToken::new();
    let hook_entered = Arc::clone(&entered);
    let hook_release = Arc::clone(&release);
    let mut options = StreamOptions {
        signal: Some(signal.clone()),
        ..Default::default()
    };
    options.callbacks.on_payload = Some(Arc::new(move |_, _| {
        let entered = Arc::clone(&hook_entered);
        let release = Arc::clone(&hook_release);
        Box::pin(async move {
            entered.notify_one();
            release.notified().await;
            Ok(None)
        })
    }));
    let mut rx = AnthropicMessages.stream(&config(&server), &model(), &context(), &options);
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    assert!(server.received_requests().await.unwrap().is_empty());
    signal.cancel();
    assert!(matches!(
        rx.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    release.notify_one();
    let events = collect(rx).await;
    assert_eq!(types(&events), ["error"]);
    assert_eq!(terminal(&events).stop_reason, StopReason::Aborted);
    assert!(server.received_requests().await.unwrap().is_empty());
}

async fn stalled_response_hook(cancel: bool) {
    let server = MockServer::start().await;
    mount(&server, "event: error\ndata: stream refused\n\n").await;
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let signal = CancellationToken::new();
    let callback_entered = Arc::clone(&entered);
    let callback_release = Arc::clone(&release);
    let mut options = StreamOptions {
        signal: Some(signal.clone()),
        max_retries: Some(2),
        ..Default::default()
    };
    options.callbacks.on_response = Some(Arc::new(move |response, _| {
        assert_eq!(response.status, 200);
        assert_eq!(response.headers["x-callback"], "observed");
        let entered = Arc::clone(&callback_entered);
        let release = Arc::clone(&callback_release);
        Box::pin(async move {
            entered.notify_one();
            release.notified().await;
            Ok(())
        })
    }));
    let mut rx = AnthropicMessages.stream(&config(&server), &model(), &context(), &options);
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    if cancel {
        signal.cancel();
    }
    assert!(
        matches!(rx.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
        "neither Start nor body errors precede hook settlement"
    );
    release.notify_one();
    let events = collect(rx).await;
    assert_eq!(types(&events), ["start", "error"]);
    assert_eq!(
        terminal(&events).stop_reason,
        if cancel {
            StopReason::Aborted
        } else {
            StopReason::Error
        }
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}
#[tokio::test]
async fn callbacks_response_is_awaited_before_start_and_sse_body_errors() {
    stalled_response_hook(false).await;
}
#[tokio::test]
async fn callbacks_response_wait_is_not_raced_by_cancellation() {
    stalled_response_hook(true).await;
}

#[tokio::test]
async fn callbacks_oauth_preserves_identity_and_normalized_tools_but_drops_replaced_betas() {
    for replace in [false, true] {
        let server = MockServer::start().await;
        mount(&server, END).await;
        let mut cfg = config(&server);
        cfg.api_key = "sk-ant-oat-local-fixture-not-a-real-token".into();
        let context = normalize_context(&Context {
            system_prompt: Some("system".into()),
            messages: vec![],
            tools: Some(vec![Tool {
                name: "read".into(),
                description: "read file".into(),
                parameters: json!({"type":"object"}),
                constrained_sampling: None,
            }]),
        });
        let mut options = SimpleStreamOptions::default();
        options.stream.callbacks.on_payload = Some(Arc::new(move |payload, _| {
            assert!(payload["betas"]
                .as_array()
                .unwrap()
                .contains(&json!("oauth-2025-04-20")));
            assert!(payload["system"][0]["text"]
                .as_str()
                .unwrap()
                .contains("Claude Code"));
            assert_eq!(payload["tools"][0]["name"], "Read");
            Box::pin(async move { Ok(replace.then(|| json!({"patched":true}))) })
        }));
        let events =
            collect(AnthropicMessages.stream_simple(&cfg, &model(), &context, &options)).await;
        assert_eq!(terminal(&events).stop_reason, StopReason::Stop);
        let requests = server.received_requests().await.unwrap();
        let request = &requests[0];
        assert_eq!(
            request.headers["authorization"].to_str().unwrap(),
            format!("Bearer {}", cfg.api_key)
        );
        assert!(request.headers["user-agent"]
            .to_str()
            .unwrap()
            .starts_with("claude-cli/"));
        assert_eq!(request.headers["x-app"], "cli");
        assert!(!request.headers.contains_key("x-api-key"));
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert!(body.get("betas").is_none());
        if replace {
            assert!(!request.headers.contains_key("anthropic-beta"));
            assert_eq!(body, json!({"patched":true,"stream":true}));
        } else {
            assert!(request.headers["anthropic-beta"]
                .to_str()
                .unwrap()
                .contains("oauth-2025-04-20"));
            assert_eq!(body["tools"][0]["name"], "Read");
        }
    }
}

#[tokio::test]
async fn callbacks_header_owned_auth_and_copilot_paths_reach_the_hooks() {
    for copilot in [false, true] {
        let server = MockServer::start().await;
        mount(&server, END).await;
        let mut cfg = config(&server);
        let mut model = model();
        let mut options = StreamOptions::default();
        if copilot {
            model.provider = "github-copilot".into();
        } else {
            cfg.api_key.clear();
            options.headers = Some(BTreeMap::from([(
                "Authorization".into(),
                Some("Bearer local-gateway-only".into()),
            )]));
        }
        let called = Arc::new(Mutex::new(Vec::new()));
        let payload_called = Arc::clone(&called);
        let response_called = Arc::clone(&called);
        options.callbacks.on_payload = Some(Arc::new(move |payload, _| {
            payload_called.lock().unwrap().push("payload");
            assert_eq!(payload["model"], "claude-callback");
            Box::pin(async { Ok(None) })
        }));
        options.callbacks.on_response = Some(Arc::new(move |metadata, _| {
            response_called.lock().unwrap().push("response");
            assert_eq!(metadata.status, 200);
            Box::pin(async { Ok(()) })
        }));
        let events = collect(AnthropicMessages.stream(&cfg, &model, &context(), &options)).await;
        assert_eq!(terminal(&events).stop_reason, StopReason::Stop);
        assert_eq!(*called.lock().unwrap(), ["payload", "response"]);
        let requests = server.received_requests().await.unwrap();
        assert!(!requests[0].headers.contains_key("x-api-key"));
        assert_eq!(
            requests[0].headers["authorization"],
            if copilot {
                "Bearer local-test-only"
            } else {
                "Bearer local-gateway-only"
            }
        );
    }
}

#[tokio::test]
async fn callbacks_missing_auth_precedes_payload_and_network() {
    let server = MockServer::start().await;
    let mut cfg = config(&server);
    cfg.api_key.clear();
    let options = StreamOptions {
        callbacks: RequestCallbacks {
            on_payload: Some(Arc::new(|_, _| panic!("authentication precedes onPayload"))),
            ..Default::default()
        },
        ..Default::default()
    };
    let events = collect(AnthropicMessages.stream(&cfg, &model(), &context(), &options)).await;
    assert_eq!(types(&events), ["error"]);
    assert_eq!(
        terminal(&events).error_message.as_deref(),
        Some("No API key for provider: anthropic")
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn callbacks_unrepresentable_string_spread_fails_explicitly_before_send() {
    let server = MockServer::start().await;
    let mut options = StreamOptions::default();
    options.callbacks.on_payload =
        Some(Arc::new(|_, _| Box::pin(async { Ok(Some(json!("🙂"))) })));
    let events =
        collect(AnthropicMessages.stream(&config(&server), &model(), &context(), &options)).await;
    assert_eq!(types(&events), ["error"]);
    assert!(terminal(&events)
        .error_message
        .as_ref()
        .unwrap()
        .contains("isolated UTF-16 surrogate"));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn callbacks_simple_budget_and_adaptive_thinking_keep_both_hooks() {
    for adaptive in [false, true] {
        let server = MockServer::start().await;
        mount(&server, END).await;
        let mut model = model();
        model.reasoning = true;
        model.max_tokens = 8192;
        model.compat = Some(json!({"forceAdaptiveThinking":adaptive}));
        let called = Arc::new(Mutex::new(Vec::new()));
        let payload_called = Arc::clone(&called);
        let response_called = Arc::clone(&called);
        let mut options = SimpleStreamOptions {
            reasoning: Some(crate::ai::types::ThinkingLevel::Medium),
            ..Default::default()
        };
        options.stream.callbacks.on_payload = Some(Arc::new(move |mut payload, _| {
            assert_eq!(
                payload["thinking"]["type"],
                if adaptive { "adaptive" } else { "enabled" }
            );
            payload["hooked"] = json!(true);
            payload_called.lock().unwrap().push("payload");
            Box::pin(async move { Ok(Some(payload)) })
        }));
        options.stream.callbacks.on_response = Some(Arc::new(move |_, _| {
            response_called.lock().unwrap().push("response");
            Box::pin(async { Ok(()) })
        }));
        let events = collect(AnthropicMessages.stream_simple(
            &config(&server),
            &model,
            &context(),
            &options,
        ))
        .await;
        assert_eq!(terminal(&events).stop_reason, StopReason::Stop);
        assert_eq!(*called.lock().unwrap(), ["payload", "response"]);
        let requests = server.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["hooked"], true);
        assert_eq!(body["stream"], true);
        assert!(body.get("betas").is_none());
    }
}

#[tokio::test]
async fn callbacks_abort_in_retry_sequence_never_repeats_payload_or_calls_response() {
    let server = MockServer::start().await;
    let received = Arc::new(Notify::new());
    let notified = Arc::clone(&received);
    Mock::given(wiremock::matchers::method("POST"))
        .respond_with(move |_: &wiremock::Request| {
            notified.notify_one();
            ResponseTemplate::new(503)
                .insert_header("retry-after-ms", "30000")
                .set_body_string("denied")
        })
        .mount(&server)
        .await;
    let signal = CancellationToken::new();
    let payloads = Arc::new(Mutex::new(0));
    let called = Arc::clone(&payloads);
    let mut options = StreamOptions {
        signal: Some(signal.clone()),
        max_retries: Some(2),
        ..Default::default()
    };
    options.callbacks.on_payload = Some(Arc::new(move |_, _| {
        *called.lock().unwrap() += 1;
        Box::pin(async { Ok(None) })
    }));
    options.callbacks.on_response = Some(Arc::new(|_, _| panic!("no successful response")));
    let rx = AnthropicMessages.stream(&config(&server), &model(), &context(), &options);
    tokio::time::timeout(Duration::from_secs(5), received.notified())
        .await
        .unwrap();
    signal.cancel();
    let events = collect(rx).await;
    assert_eq!(types(&events), ["error"]);
    assert_eq!(terminal(&events).stop_reason, StopReason::Aborted);
    assert_eq!(*payloads.lock().unwrap(), 1);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}
