//! Offline loopback tests for the process-local request callbacks
//! (mistral-conversations.ts:147 onPayload, :311 onResponse): the seam
//! semantics pinned by `tests/fixtures/callback_seam_oracle` — a replacement reaches
//! the wire, onResponse fires after the fetch resolves and BEFORE the ok
//! check (the hook observes non-success responses too), and hook rejections
//! take the pre-send (payload) / pre-start (response) error paths.
use super::*;
use crate::ai::transcript::{normalize_context, Context};
use crate::ai::types::message::AssistantMessage;
use crate::ai::types::primitives::ModelCost;
use crate::ai::types::{Model, ModelInput};
use serde_json::json;
use std::sync::{Arc, Mutex};

fn model(server: &wiremock::MockServer) -> Model {
    Model {
        r#type: None,
        prompt_cache: None,
        input_limits: None,
        id: "mistral-large-latest".to_string(),
        name: "Mistral Large".to_string(),
        api: API.to_string(),
        provider: "mistral".to_string(),
        base_url: server.uri(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![ModelInput::Text],
        cost: ModelCost::default(),
        context_window: 128000,
        max_tokens: 8192,
        sampling_params: None,
        sampling_params_by_thinking_level: None,
        headers: None,
        compat: None,
    }
}

fn ctx() -> TranscriptContext {
    normalize_context(&Context {
        system_prompt: None,
        messages: vec![],
        tools: None,
    })
}

fn config() -> ProviderConfig {
    ProviderConfig {
        base_url: "https://unused.example.com".to_string(),
        api_key: "test-api-key".to_string(),
        max_tokens: 8192,
    }
}

fn content_chunk(text: &str) -> Value {
    json!({
        "id": "chunk",
        "model": "mistral-large-latest",
        "choices": [{"index": 0, "finish_reason": Value::Null, "delta": {"content": text}}],
    })
}

fn terminal_event() -> Value {
    json!({
        "id": "mistral-response-id",
        "model": "mistral-large-latest",
        "choices": [{"index": 0, "finish_reason": "stop", "delta": {}}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
    })
}

fn sse(events: &[Value]) -> wiremock::ResponseTemplate {
    let mut body = String::new();
    for event in events {
        body.push_str(&format!("data: {event}\r\n\r\n"));
    }
    body.push_str("data: [DONE]\r\n\r\n");
    wiremock::ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body)
}

async fn mount(server: &wiremock::MockServer) {
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/chat/completions"))
        .respond_with(sse(&[content_chunk("hi"), terminal_event()]))
        .mount(server)
        .await;
}

async fn collect(mut rx: mpsc::Receiver<AssistantMessageEvent>) -> Vec<AssistantMessageEvent> {
    let mut out = Vec::new();
    while let Some(event) = rx.recv().await {
        out.push(event);
    }
    out
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

/// Both hooks observe the real generation in the upstream order — onPayload
/// before the send (its replacement reaches the wire body), onResponse after
/// the fetch resolves and before `start` — and the simple path carries the
/// callbacks identically (upstream streamSimple delegates to `stream` through
/// `buildBaseOptions`, lines 186-210).
#[tokio::test]
async fn callbacks_replace_the_wire_body_and_order_lifecycle_around_start() {
    let server = wiremock::MockServer::start().await;
    mount(&server).await;
    let stream_model = model(&server);
    let trace = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut options = StreamOptions::default();
    let payload_trace = Arc::clone(&trace);
    options.callbacks.on_payload = Some(Arc::new(move |mut payload, hook_model| {
        assert_eq!(hook_model.id, "mistral-large-latest");
        assert_eq!(payload["model"], "mistral-large-latest");
        payload_trace.lock().unwrap().push("payload".into());
        payload["hooked"] = json!(true);
        Box::pin(async move { Ok(Some(payload)) })
    }));
    let response_trace = Arc::clone(&trace);
    options.callbacks.on_response = Some(Arc::new(move |response, _| {
        let mut trace = response_trace.lock().unwrap();
        assert_eq!(response.status, 200);
        assert!(
            !response.headers.is_empty(),
            "the hook receives the raw header record"
        );
        trace.push("response".into());
        Box::pin(async { Ok(()) })
    }));

    let rx = MistralConversations.stream(&config(), &stream_model, &ctx(), &options);
    let events = tokio::time::timeout(std::time::Duration::from_secs(5), collect(rx))
        .await
        .expect("bounded");
    assert_eq!(
        types(&events),
        ["start", "text_start", "text_delta", "text_end", "done"]
    );
    assert_eq!(
        *trace.lock().unwrap(),
        ["payload".to_string(), "response".to_string()],
        "onResponse precedes start and onPayload precedes the send"
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["hooked"], true, "the replacement reaches the wire");

    // The simple path carries the same hooks to the same seams.
    let server = wiremock::MockServer::start().await;
    mount(&server).await;
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let payload_seen = Arc::clone(&seen);
    let mut simple = SimpleStreamOptions::default();
    simple.stream.callbacks.on_payload = Some(Arc::new(move |mut payload, _| {
        payload_seen.lock().unwrap().push("payload".into());
        payload["hooked"] = json!(true);
        Box::pin(async move { Ok(Some(payload)) })
    }));
    let response_seen = Arc::clone(&seen);
    simple.stream.callbacks.on_response = Some(Arc::new(move |_, _| {
        response_seen.lock().unwrap().push("response".into());
        Box::pin(async { Ok(()) })
    }));
    let rx = MistralConversations.stream_simple(&config(), &model(&server), &ctx(), &simple);
    let events = tokio::time::timeout(std::time::Duration::from_secs(5), collect(rx))
        .await
        .expect("bounded");
    assert_eq!(
        types(&events),
        ["start", "text_start", "text_delta", "text_end", "done"]
    );
    assert_eq!(
        *seen.lock().unwrap(),
        ["payload".to_string(), "response".to_string()]
    );
    let body: Value =
        serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
    assert_eq!(body["hooked"], true);
}

/// The oracle's pre-ok-check ordering (mistral-conversations.ts:311-316): a
/// non-success response still reaches onResponse — the hook observes the 429
/// before the HTTP error throws into the catch block.
#[tokio::test]
async fn onresponse_observes_non_success_responses_before_the_http_error() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/chat/completions"))
        .respond_with(
            wiremock::ResponseTemplate::new(429)
                .insert_header("content-type", "text/plain")
                .set_body_string("denied"),
        )
        .mount(&server)
        .await;
    let statuses = Arc::new(Mutex::new(Vec::<u16>::new()));
    let hook_statuses = Arc::clone(&statuses);
    let mut options = StreamOptions::default();
    options.callbacks.on_response = Some(Arc::new(move |response, _| {
        hook_statuses.lock().unwrap().push(response.status);
        Box::pin(async { Ok(()) })
    }));
    let rx = MistralConversations.stream(&config(), &model(&server), &ctx(), &options);
    let events = tokio::time::timeout(std::time::Duration::from_secs(5), collect(rx))
        .await
        .expect("bounded");
    assert_eq!(types(&events), ["error"]);
    assert_eq!(
        *statuses.lock().unwrap(),
        [429],
        "the hook saw the 429 before the ok check"
    );
    assert_eq!(terminal(&events).stop_reason, StopReason::Error);
    assert_eq!(
        terminal(&events).error_message.as_deref(),
        Some("Mistral API error (429): denied")
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

/// A payload-hook rejection throws into the catch block before any HTTP
/// traffic (upstream lines 147/168-177).
#[tokio::test]
async fn callbacks_payload_failure_is_pre_send() {
    let server = wiremock::MockServer::start().await;
    mount(&server).await;
    let mut options = StreamOptions::default();
    options.callbacks.on_payload = Some(Arc::new(|_, _| {
        Box::pin(async { anyhow::bail!("payload refused") })
    }));
    let rx = MistralConversations.stream(&config(), &model(&server), &ctx(), &options);
    let events = tokio::time::timeout(std::time::Duration::from_secs(5), collect(rx))
        .await
        .expect("bounded");
    assert_eq!(types(&events), ["error"]);
    assert_eq!(terminal(&events).stop_reason, StopReason::Error);
    assert_eq!(
        terminal(&events).error_message.as_deref(),
        Some("payload refused")
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

/// A response-hook rejection takes the same catch path after the send: the
/// server saw the request, `start` never fires (upstream line 311 precedes
/// the `start` push at line 152).
#[tokio::test]
async fn callbacks_response_failure_is_pre_start_post_send() {
    let server = wiremock::MockServer::start().await;
    mount(&server).await;
    let mut options = StreamOptions::default();
    options.callbacks.on_response = Some(Arc::new(|_, _| {
        Box::pin(async { anyhow::bail!("response refused") })
    }));
    let rx = MistralConversations.stream(&config(), &model(&server), &ctx(), &options);
    let events = tokio::time::timeout(std::time::Duration::from_secs(5), collect(rx))
        .await
        .expect("bounded");
    assert_eq!(types(&events), ["error"]);
    assert_eq!(terminal(&events).stop_reason, StopReason::Error);
    assert_eq!(
        terminal(&events).error_message.as_deref(),
        Some("response refused")
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

/// Both upstream hook sites are wired: the capability gate admits
/// callback-bearing requests.
#[test]
fn supports_request_callbacks_matches_upstream() {
    assert!(MistralConversations.supports_request_callbacks());
}
