//! Offline real HTTP tests for the process-local OpenAI Responses request
//! callbacks: the actual-source lifecycle oracle replayed through the real
//! normal and simple adapters over loopback HTTP, plus hook-gate, abort and
//! Models-routing tests.
use super::*;
use crate::ai::auth::types::{AuthContext, ProviderAuth};
use crate::ai::models::provider::{create_provider, ApiImpls, CreateProviderOptions};
use crate::ai::models::{create_models, CreateModelsOptions, ModelsSimpleStreamOptions};
use crate::ai::transcript::{normalize_context, Context};
use crate::ai::types::message::AssistantMessage;
use crate::ai::types::request_callbacks::RequestCallbacks;
use futures::future::BoxFuture;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;
use wiremock::{Mock, MockServer, ResponseTemplate};

const COMPLETED_SSE: &[u8] = concat!(
    r#"data: {"type":"response.completed","response":{"status":"completed"}}"#,
    "\n\n"
)
.as_bytes();
const SSE_ERROR: &[u8] = concat!(
    r#"data: {"type":"error","code":"server_error","message":"stream refused"}"#,
    "\n\n"
)
.as_bytes();
const NONSTREAM_BODY: &str = r#"{"object":"response","output":[],"usage":{}}"#;
const DENIED_BODY: &[u8] = br#"{"denied":true}"#;

fn model() -> Model {
    Model {
        r#type: None,
        prompt_cache: None,
        input_limits: None,
        id: "gpt-5.4".into(),
        name: "GPT-5.4".into(),
        api: "openai-responses".into(),
        provider: "openai".into(),
        base_url: "https://api.openai.com/v1".into(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![crate::ai::types::ModelInput::Text],
        cost: crate::ai::types::primitives::ModelCost::default(),
        context_window: 400000,
        max_tokens: 128000,
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
        // No /v1 suffix: the wire path is {base_url}/responses, matching the
        // oracle's recorded "/responses" URL.
        base_url: server.uri(),
        api_key: "local-test-only".into(),
        max_tokens: 8192,
    }
}
async fn mount(server: &MockServer, body: &'static [u8]) {
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(body, "text/event-stream")
                .insert_header("x-callback", "observed"),
        )
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
        let entry = if simple { "simple" } else { "normal" };
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
        let response_body: String = input
            .get("responseBody")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| NONSTREAM_BODY.to_string());
        let sse_error = input["sseError"] == true;
        // The canned transport reacts to the request's own stream mode, like
        // the oracle transport reading the SDK-computed `body.stream ?? false`.
        Mock::given(wiremock::matchers::method("POST"))
            .respond_with(move |request: &wiremock::Request| {
                request_trace.lock().unwrap().push("request".into());
                let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
                let streaming = js_stream_mode(body.get("stream"));
                let mut log = request_log.lock().unwrap();
                let status = statuses.get(log.len()).copied().unwrap_or(200);
                log.push(json!({"url": request.url.path(), "body": body}));
                if status == 200 {
                    if streaming && sse_error {
                        ResponseTemplate::new(200)
                            .set_body_raw(SSE_ERROR, "text/event-stream")
                            .insert_header("x-callback", "observed")
                    } else if streaming {
                        ResponseTemplate::new(200)
                            .set_body_raw(COMPLETED_SSE, "text/event-stream")
                            .insert_header("x-callback", "observed")
                    } else {
                        ResponseTemplate::new(200)
                            .set_body_raw(response_body.as_bytes(), "application/json")
                            .insert_header("x-callback", "observed")
                    }
                } else {
                    ResponseTemplate::new(status)
                        .insert_header("retry-after-ms", "0")
                        .set_body_raw(DENIED_BODY, "application/json")
                }
            })
            .mount(&server)
            .await;
        let signal = CancellationToken::new();
        let mut options = StreamOptions {
            signal: Some(signal.clone()),
            max_retries: Some(2),
            ..Default::default()
        };
        if input["cancel"] == "before" {
            signal.cancel();
        }
        if input.get("payload").map(|v| v != "absent").unwrap_or(false) {
            let trace = Arc::clone(&trace);
            let seen = Arc::clone(&seen);
            let cancel = input["cancel"] == "payload";
            let fail = input["payload"] == "error";
            let replacement = input.get("replacement").cloned();
            let signal = signal.clone();
            options.callbacks.on_payload = Some(Arc::new(move |payload, model| {
                assert_eq!(model.id, "gpt-5.4");
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
        if input.get("response").map(|v| v != false).unwrap_or(false) {
            let trace = Arc::clone(&trace);
            let metadata = Arc::clone(&metadata);
            let cancel = input["cancel"] == "response";
            let fail = input["response"] == "error";
            let signal = signal.clone();
            options.callbacks.on_response = Some(Arc::new(move |response, model| {
                assert_eq!(model.id, "gpt-5.4");
                assert_eq!(response.status, 200);
                assert_eq!(response.headers["x-callback"], "observed");
                trace.lock().unwrap().push("response".into());
                metadata.lock().unwrap().push(
                    json!({"status": response.status, "headers": {"content-type": response.headers.get("content-type"), "x-callback": response.headers.get("x-callback")}}),
                );
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
        let api = OpenAiResponses;
        let mut rx = if simple {
            api.stream_simple(
                &config(&server),
                &model(),
                &context(),
                &SimpleStreamOptions {
                    stream: options,
                    ..SimpleStreamOptions::default()
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
        assert_eq!(
            actual, case["output"][entry],
            "{} simple={simple}",
            input["name"]
        );
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
    mount(&server, COMPLETED_SSE).await;
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
    let mut rx = OpenAiResponses.stream(&config(&server), &model(), &context(), &options);
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
    mount(&server, SSE_ERROR).await;
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
    let mut rx = OpenAiResponses.stream(&config(&server), &model(), &context(), &options);
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
async fn callbacks_missing_auth_precedes_payload_and_network() {
    let server = MockServer::start().await;
    let mut request_cfg = config(&server);
    request_cfg.api_key.clear();
    let mut model = model();
    model.provider = "some-proxy".into();
    let options = StreamOptions {
        callbacks: RequestCallbacks {
            on_payload: Some(Arc::new(|_, _| panic!("authentication precedes onPayload"))),
            ..Default::default()
        },
        ..Default::default()
    };
    let events = collect(OpenAiResponses.stream(&request_cfg, &model, &context(), &options)).await;
    assert_eq!(types(&events), ["error"]);
    assert_eq!(
        terminal(&events).error_message.as_deref(),
        Some("No API key for provider: some-proxy")
    );
    assert!(server.received_requests().await.unwrap().is_empty());
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
                .set_body_raw(DENIED_BODY, "application/json")
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
    let rx = OpenAiResponses.stream(&config(&server), &model(), &context(), &options);
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

struct EnvAuthContext {
    vars: BTreeMap<String, String>,
}

impl AuthContext for EnvAuthContext {
    fn env<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move { self.vars.get(name).cloned() })
    }

    fn file_exists<'a>(&'a self, _path: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async { false })
    }
}

/// Real Models routing: a callback-bearing request must reach the Responses
/// adapter (capability gate passes) and both hooks must observe the real
/// generation over loopback HTTP.
#[tokio::test]
async fn models_generation_routes_callback_bearing_requests_through_the_responses_adapter() {
    let server = MockServer::start().await;
    mount(&server, COMPLETED_SSE).await;
    let mut models = create_models(CreateModelsOptions {
        auth_context: Some(Arc::new(EnvAuthContext {
            vars: [("P1_API_KEY".to_string(), "env-key".to_string())]
                .into_iter()
                .collect(),
        }) as Arc<dyn AuthContext>),
        credentials: None,
        models_store: None,
    });
    let mut model = model();
    model.provider = "p1".into();
    model.base_url = server.uri();
    let per_api: BTreeMap<String, Arc<dyn ApiImpl>> = BTreeMap::from([(
        "openai-responses".to_string(),
        Arc::new(OpenAiResponses) as Arc<dyn ApiImpl>,
    )]);
    models.set_provider(create_provider(CreateProviderOptions {
        filter_all_models: None,
        images: crate::ai::models::provider::ImagesImpls::new(),
        classifiers: crate::ai::models::provider::ClassifiersImpls::new(),
        id: "p1".into(),
        name: None,
        base_url: None,
        headers: None,
        auth: ProviderAuth {
            api_key: Some(crate::ai::auth::helpers::env_api_key_auth(
                "P1",
                &["P1_API_KEY"],
            )),
            oauth: None,
        },
        models: vec![crate::ai::types::AnyModel::Chat(model.clone())],
        fetch_models: None,
        filter_models: None,
        api: ApiImpls::PerApi(per_api),
    }));
    let called = Arc::new(Mutex::new(Vec::new()));
    let payload_called = Arc::clone(&called);
    let response_called = Arc::clone(&called);
    let mut simple = SimpleStreamOptions::default();
    simple.stream.callbacks.on_payload = Some(Arc::new(move |mut payload, _| {
        assert_eq!(payload["model"], "gpt-5.4");
        payload["hooked"] = json!(true);
        payload_called.lock().unwrap().push("payload");
        Box::pin(async move { Ok(Some(payload)) })
    }));
    simple.stream.callbacks.on_response = Some(Arc::new(move |metadata, _| {
        assert_eq!(metadata.status, 200);
        response_called.lock().unwrap().push("response");
        Box::pin(async { Ok(()) })
    }));
    let message = models
        .complete_simple(
            &model,
            &Context {
                system_prompt: None,
                messages: vec![],
                tools: None,
            },
            Some(ModelsSimpleStreamOptions {
                simple,
                transform_headers: None,
            }),
        )
        .await;
    assert_eq!(
        message.stop_reason,
        StopReason::Stop,
        "message: {} {message:?}",
        message.error_message.as_deref().unwrap_or_default()
    );
    assert_eq!(*called.lock().unwrap(), ["payload", "response"]);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].headers["authorization"].to_str().unwrap(),
        "Bearer env-key"
    );
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["hooked"], true);
    assert_eq!(body["stream"], true);
    assert_eq!(body["max_output_tokens"], json!(128000));
    server.verify().await;
}
