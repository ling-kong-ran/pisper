//! Offline loopback tests for the process-local request callbacks of the
//! Google Generative AI adapter. Upstream wires exactly one hook —
//! `options?.onPayload?.(params, model)` (google-generative-ai.ts:96) — and
//! has no onResponse call site, so a callback-bearing request carries the
//! payload replacement to the wire and a response hook is silently unused
//! (never invoked, never rejected, stream succeeds).
use super::*;
use crate::ai::transcript::{normalize_context, Context};
use crate::ai::types::primitives::ModelCost;
use crate::ai::types::{Model, ModelInput};
use serde_json::json;
use std::sync::{Arc, Mutex};

fn model(base_url: &str) -> Model {
    Model {
        r#type: None,
        prompt_cache: None,
        input_limits: None,
        id: "gemini-2.5-flash".to_string(),
        name: "Gemini 2.5 Flash".to_string(),
        api: API.to_string(),
        provider: "google".to_string(),
        base_url: base_url.to_string(),
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

fn sse(chunks: &[Value]) -> wiremock::ResponseTemplate {
    let body: String = chunks
        .iter()
        .map(|chunk| format!("data: {chunk}\n\n"))
        .collect();
    wiremock::ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body)
}

async fn mount(server: &wiremock::MockServer) {
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(
            "/v1beta/models/gemini-2.5-flash:streamGenerateContent",
        ))
        .respond_with(sse(&[
            json!({"candidates": [{"content": {"parts": [{"text": "hi"}], "role": "model"}}]}),
            json!({"candidates": [{"finishReason": "STOP"}]}),
        ]))
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

/// The payload hook's replacement reaches the wire body; the response hook is
/// never invoked (upstream google-generative-ai.ts has no onResponse call
/// site) and its presence neither fails nor delays the stream.
#[tokio::test]
async fn payload_hook_replaces_the_wire_body_and_onresponse_is_silently_unused() {
    let server = wiremock::MockServer::start().await;
    mount(&server).await;
    let model = model(&format!("{}/v1beta", server.uri()));
    let mut options = StreamOptions::default();
    let called = Arc::new(Mutex::new(Vec::<String>::new()));
    let payload_called = Arc::clone(&called);
    options.callbacks.on_payload = Some(Arc::new(move |mut payload, hook_model| {
        assert_eq!(hook_model.id, "gemini-2.5-flash");
        payload_called.lock().unwrap().push("payload".into());
        payload["hooked"] = json!(true);
        Box::pin(async move { Ok(Some(payload)) })
    }));
    let response_called = Arc::clone(&called);
    options.callbacks.on_response = Some(Arc::new(move |_, _| {
        response_called.lock().unwrap().push("response".into());
        Box::pin(async { Ok(()) })
    }));

    let rx = GoogleGenerativeAi.stream(&config(), &model, &ctx(), &options);
    let events = tokio::time::timeout(std::time::Duration::from_secs(5), collect(rx))
        .await
        .expect("bounded");
    assert_eq!(
        types(&events),
        ["start", "text_start", "text_delta", "text_end", "done"]
    );
    assert_eq!(
        *called.lock().unwrap(),
        ["payload".to_string()],
        "onResponse is never invoked: upstream has no call site"
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["hooked"], true, "the replacement reaches the wire");
    assert_eq!(terminal(&events).stop_reason, StopReason::Stop);
}

/// The oracle's replacement rule (google-generative-ai.ts:96-99): a hook
/// returning `undefined` (the port's `None`) keeps the built params.
#[tokio::test]
async fn payload_hook_none_preserves_the_built_body() {
    let server = wiremock::MockServer::start().await;
    mount(&server).await;
    let model = model(&format!("{}/v1beta", server.uri()));
    let mut options = StreamOptions::default();
    options.callbacks.on_payload = Some(Arc::new(|_, _| Box::pin(async { Ok(None) })));
    let rx = GoogleGenerativeAi.stream(&config(), &model, &ctx(), &options);
    let events = tokio::time::timeout(std::time::Duration::from_secs(5), collect(rx))
        .await
        .expect("bounded");
    assert_eq!(
        types(&events),
        ["start", "text_start", "text_delta", "text_end", "done"]
    );
    let body: Value =
        serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
    assert_eq!(body["hooked"], Value::Null);
    assert_eq!(
        body["contents"],
        json!([]),
        "no messages: the built body keeps its shape"
    );
}

/// A payload-hook rejection throws into the catch block before any HTTP
/// traffic (upstream lines 96/287-298).
#[tokio::test]
async fn payload_hook_failure_is_pre_send() {
    let server = wiremock::MockServer::start().await;
    mount(&server).await;
    let model = model(&format!("{}/v1beta", server.uri()));
    let mut options = StreamOptions::default();
    options.callbacks.on_payload = Some(Arc::new(|_, _| {
        Box::pin(async { anyhow::bail!("payload refused") })
    }));
    let rx = GoogleGenerativeAi.stream(&config(), &model, &ctx(), &options);
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

/// The single upstream hook site is wired: the capability gate admits
/// callback-bearing requests.
#[test]
fn supports_request_callbacks_matches_upstream() {
    assert!(GoogleGenerativeAi.supports_request_callbacks());
}
