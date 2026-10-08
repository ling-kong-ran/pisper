// Test fixtures intentionally mutate a Default-built callbacks record field by
// field, mirroring the upstream tests' incremental wiring style.
#![allow(clippy::field_reassign_with_default)]
//! Offline loopback tests for the process-local request callbacks
//! (openai-codex-responses.ts:278 onPayload, :414 onResponse): the seam
//! semantics pinned by `tests/fixtures/callback_seam_oracle` — a replacement reaches
//! the wire body (and, by seam placement, feeds the websocket transport too,
//! which has no onResponse call site), onResponse fires per SSE attempt
//! before the ok check, and a response-hook rejection retries like any other
//! attempt error before surfacing.
use super::*;
use crate::ai::transcript::{normalize_context, Context};
use crate::ai::types::message::{AssistantMessage, Message, StringOrBlocks, UserMessage};
use crate::ai::types::primitives::ModelCost;
use crate::ai::types::request_callbacks::RequestCallbacks;
use crate::ai::types::{Model, ModelInput};
use serde_json::json;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

const TS: i64 = 1758240000000;

/// A JWT-shaped bearer token carrying the ChatGPT account claim.
fn mock_token(account_id: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let encode = |input: &[u8]| -> String {
        let mut out = String::new();
        for chunk in input.chunks(3) {
            let mut bytes = [0u8; 3];
            bytes[..chunk.len()].copy_from_slice(chunk);
            let packed = ((bytes[0] as u32) << 16) | ((bytes[1] as u32) << 8) | bytes[2] as u32;
            out.push(ALPHABET[(packed >> 18) as usize & 63] as char);
            out.push(ALPHABET[(packed >> 12) as usize & 63] as char);
            if chunk.len() > 1 {
                out.push(ALPHABET[(packed >> 6) as usize & 63] as char);
            }
            if chunk.len() > 2 {
                out.push(ALPHABET[packed as usize & 63] as char);
            }
        }
        while !out.len().is_multiple_of(4) {
            out.push('=');
        }
        out
    };
    let payload = encode(
        &serde_json::to_vec(&json!({ JWT_CLAIM_PATH: { "chatgpt_account_id": account_id } }))
            .unwrap(),
    );
    format!("aaa.{payload}.bbb")
}

fn model_on(server: &wiremock::MockServer) -> Model {
    Model {
        r#type: None,
        prompt_cache: None,
        input_limits: None,
        id: "gpt-5.1-codex".to_string(),
        name: "GPT-5.1 Codex".to_string(),
        api: "openai-codex-responses".to_string(),
        provider: "openai-codex".to_string(),
        base_url: server.uri(),
        reasoning: true,
        thinking_level_map: None,
        input: vec![ModelInput::Text],
        cost: ModelCost::default(),
        context_window: 400000,
        max_tokens: 128000,
        sampling_params: None,
        sampling_params_by_thinking_level: None,
        headers: None,
        compat: None,
    }
}

fn config() -> ProviderConfig {
    ProviderConfig {
        base_url: "https://chatgpt.com/backend-api".to_string(),
        api_key: String::new(),
        max_tokens: 8192,
    }
}

fn ctx() -> TranscriptContext {
    normalize_context(&Context {
        system_prompt: None,
        messages: vec![Message::User(UserMessage {
            content: StringOrBlocks::Text("hello".to_string()),
            timestamp: TS,
        })],
        tools: None,
    })
}

fn completed_sse() -> String {
    let response = json!({
        "status": "completed",
        "usage": {"input_tokens": 5, "output_tokens": 3, "total_tokens": 8},
    });
    [
        json!({
            "type": "response.output_item.added",
            "item": {"type": "message", "id": "msg_1", "role": "assistant", "status": "in_progress", "content": []},
        }),
        json!({"type": "response.output_text.delta", "delta": "Hello"}),
        json!({"type": "response.completed", "response": response}),
    ]
    .iter()
    .map(|event| format!("data: {event}\n\n"))
    .collect()
}

fn sse_options(callbacks: RequestCallbacks, max_retries: u32) -> SimpleStreamOptions {
    SimpleStreamOptions {
        stream: StreamOptions {
            api_key: Some(mock_token("acc_test")),
            transport: Some(Transport::Sse),
            max_retries: Some(max_retries),
            callbacks,
            ..Default::default()
        },
        ..SimpleStreamOptions::default()
    }
}

async fn mount_codex(server: &wiremock::MockServer, status: u16, body: String) {
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/codex/responses"))
        .respond_with(
            wiremock::ResponseTemplate::new(status)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
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

fn types(events: &[AssistantMessageEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(AssistantMessageEvent::event_type)
        .collect()
}

fn terminal(events: &[AssistantMessageEvent]) -> &AssistantMessage {
    match events.last().expect("terminal event") {
        AssistantMessageEvent::Done { message, .. } => message,
        AssistantMessageEvent::Error { error, .. } => error,
        other => panic!("non-terminal {other:?}"),
    }
}

/// The SSE path zstd-compresses the body when the codec is available; decode
/// it back before asserting.
fn decode_request_body(request: &wiremock::Request) -> Value {
    if request
        .headers
        .get("content-encoding")
        .map(|value| value == "zstd")
        .unwrap_or(false)
    {
        let decoded = zstd::stream::decode_all(&request.body[..]).unwrap();
        serde_json::from_slice(&decoded).unwrap()
    } else {
        serde_json::from_slice(&request.body).unwrap()
    }
}

/// Both hooks observe the real generation in the upstream order — onPayload
/// before the transport selection (its replacement reaches the wire body),
/// onResponse after the fetch resolves and before `start` — and the stream
/// completes.
#[tokio::test]
async fn callbacks_replace_the_wire_body_and_order_lifecycle_around_start() {
    let server = wiremock::MockServer::start().await;
    mount_codex(&server, 200, completed_sse()).await;
    let trace = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut callbacks = RequestCallbacks::default();
    let payload_trace = Arc::clone(&trace);
    callbacks.on_payload = Some(Arc::new(move |mut payload, hook_model| {
        assert_eq!(hook_model.id, "gpt-5.1-codex");
        payload_trace.lock().unwrap().push("payload".into());
        payload["hooked"] = json!(true);
        Box::pin(async move { Ok(Some(payload)) })
    }));
    let response_trace = Arc::clone(&trace);
    callbacks.on_response = Some(Arc::new(move |response, _| {
        let mut trace = response_trace.lock().unwrap();
        assert_eq!(response.status, 200);
        assert!(
            !response.headers.is_empty(),
            "the hook receives the raw header record"
        );
        trace.push("response".into());
        Box::pin(async { Ok(()) })
    }));

    let rx = OpenAiCodexResponses.stream_simple(
        &config(),
        &model_on(&server),
        &ctx(),
        &sse_options(callbacks, 0),
    );
    let events = tokio::time::timeout(std::time::Duration::from_secs(5), collect(rx))
        .await
        .expect("bounded");
    assert_eq!(*types(&events).first().unwrap(), "start");
    assert_eq!(types(&events).last().copied(), Some("done"));
    assert_eq!(
        *trace.lock().unwrap(),
        ["payload".to_string(), "response".to_string()],
        "onResponse precedes start and onPayload precedes the send"
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body: Value = decode_request_body(&requests[0]);
    assert_eq!(body["hooked"], true, "the replacement reaches the wire");
    assert_eq!(terminal(&events).stop_reason, StopReason::Stop);
}

/// The oracle's per-attempt ordering (openai-codex-responses.ts:414-421): a
/// retriable 429 reaches onResponse, the retry succeeds, and the hook has
/// fired once per attempt — before each ok check.
#[tokio::test]
async fn onresponse_fires_per_attempt_before_the_ok_check() {
    let server = wiremock::MockServer::start().await;
    let attempts = Arc::new(AtomicU32::new(0));
    let attempt_count = Arc::clone(&attempts);
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/codex/responses"))
        .respond_with(move |_request: &wiremock::Request| {
            if attempt_count.fetch_add(1, Ordering::SeqCst) == 0 {
                wiremock::ResponseTemplate::new(429)
                    .insert_header("retry-after-ms", "0")
                    .set_body_string("rate limited")
            } else {
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(completed_sse())
            }
        })
        .mount(&server)
        .await;
    let statuses = Arc::new(Mutex::new(Vec::<u16>::new()));
    let hook_statuses = Arc::clone(&statuses);
    let mut callbacks = RequestCallbacks::default();
    callbacks.on_response = Some(Arc::new(move |response, _| {
        hook_statuses.lock().unwrap().push(response.status);
        Box::pin(async { Ok(()) })
    }));
    let rx = OpenAiCodexResponses.stream_simple(
        &config(),
        &model_on(&server),
        &ctx(),
        &sse_options(callbacks, 2),
    );
    let events = tokio::time::timeout(std::time::Duration::from_secs(5), collect(rx))
        .await
        .expect("bounded");
    assert_eq!(types(&events).last().copied(), Some("done"));
    assert_eq!(
        *statuses.lock().unwrap(),
        [429, 200],
        "one onResponse call per attempt"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
    assert_eq!(terminal(&events).stop_reason, StopReason::Stop);
}

/// A response-hook rejection becomes the attempt error and rides the attempt
/// catch block (openai-codex-responses.ts:442-459): it retries like any other
/// non-"usage limit" error and then surfaces with the hook's message.
#[tokio::test]
async fn response_hook_failure_retries_then_throws() {
    let server = wiremock::MockServer::start().await;
    mount_codex(&server, 200, completed_sse()).await;
    let mut callbacks = RequestCallbacks::default();
    callbacks.on_response = Some(Arc::new(|_, _| {
        Box::pin(async { anyhow::bail!("response refused") })
    }));
    let rx = OpenAiCodexResponses.stream_simple(
        &config(),
        &model_on(&server),
        &ctx(),
        &sse_options(callbacks, 1),
    );
    let events = tokio::time::timeout(std::time::Duration::from_secs(5), collect(rx))
        .await
        .expect("bounded");
    assert_eq!(types(&events), ["error"]);
    assert_eq!(terminal(&events).stop_reason, StopReason::Error);
    assert_eq!(
        terminal(&events).error_message.as_deref(),
        Some("response refused")
    );
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        2,
        "the hook rejection retried once before surfacing"
    );
}

/// A payload-hook rejection throws into the catch block before any transport
/// traffic — no websocket dial, no SSE fetch (upstream lines 278/484-494).
#[tokio::test]
async fn callbacks_payload_failure_is_pre_transport() {
    let server = wiremock::MockServer::start().await;
    mount_codex(&server, 200, completed_sse()).await;
    let mut callbacks = RequestCallbacks::default();
    callbacks.on_payload = Some(Arc::new(|_, _| {
        Box::pin(async { anyhow::bail!("payload refused") })
    }));
    let rx = OpenAiCodexResponses.stream_simple(
        &config(),
        &model_on(&server),
        &ctx(),
        &sse_options(callbacks, 0),
    );
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

/// Both upstream hook sites are wired: the capability gate admits
/// callback-bearing requests.
#[test]
fn supports_request_callbacks_matches_upstream() {
    assert!(OpenAiCodexResponses.supports_request_callbacks());
}
