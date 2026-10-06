// Test fixtures intentionally mutate a Default-built callbacks record field by
// field, mirroring the upstream tests' incremental wiring style.
#![allow(clippy::field_reassign_with_default)]
//! Offline loopback tests for the process-local request callbacks
//! (bedrock-converse-stream.ts:280 onPayload, :251-255/:510-526 onResponse):
//! the seam semantics pinned by `tests/fixtures/callback_seam_oracle` — a
//! replacement reaches the wire body (the replacement is serialized and
//! signed, since upstream replaces the command input before the command is
//! built), onResponse fires once the send resolves with the raw HTTP response
//! and before the event stream is consumed, and hook rejections take the
//! pre-send (payload) / pre-stream (response) error paths.
use super::*;
use crate::ai::api::bedrock::event_stream::crc32;
use crate::ai::api::test_support::TestEnv;
use crate::ai::transcript::{normalize_context, Context};
use crate::ai::types::message::{AssistantMessage, Message, StringOrBlocks, UserMessage};
use crate::ai::types::primitives::ModelCost;
use crate::ai::types::request_callbacks::RequestCallbacks;
use crate::ai::types::{Model, ModelInput};
use serde_json::json;
use std::sync::{Arc, Mutex};

const TS: i64 = 1758240000000;

/// Holds the env lock with the AWS ambient vars cleared: resolution falls
/// through to the scoped map carried on the options (hermetic on machines
/// exporting real AWS_* variables).
fn cleared_aws_env() -> TestEnv {
    TestEnv::apply(
        &[],
        &[
            "AWS_PROFILE",
            "AWS_REGION",
            "AWS_DEFAULT_REGION",
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
            "AWS_BEARER_TOKEN_BEDROCK",
            "AWS_BEDROCK_SKIP_AUTH",
        ],
    )
}

fn model() -> Model {
    Model {
        r#type: None,
        prompt_cache: None,
        input_limits: None,
        id: "us.anthropic.claude-sonnet-4-5-20250929-v1:0".to_string(),
        name: "Claude Sonnet 4.5 (US)".to_string(),
        api: API.to_string(),
        provider: "amazon-bedrock".to_string(),
        base_url: "https://bedrock-runtime.us-east-1.amazonaws.com".to_string(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![ModelInput::Text],
        cost: ModelCost::default(),
        context_window: 200000,
        max_tokens: 64000,
        sampling_params: None,
        sampling_params_by_thinking_level: None,
        headers: None,
        compat: None,
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

fn config() -> ProviderConfig {
    ProviderConfig {
        base_url: "https://bedrock-runtime.us-east-1.amazonaws.com".to_string(),
        api_key: String::new(),
        max_tokens: 4096,
    }
}

fn signing_env() -> ProviderEnv {
    [
        ("AWS_ACCESS_KEY_ID".to_string(), "AKIDEXAMPLE".to_string()),
        (
            "AWS_SECRET_ACCESS_KEY".to_string(),
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".to_string(),
        ),
        ("AWS_REGION".to_string(), "us-east-1".to_string()),
    ]
    .into_iter()
    .collect()
}

fn options_with(callbacks: RequestCallbacks) -> StreamOptions {
    StreamOptions {
        env: Some(signing_env()),
        callbacks,
        ..StreamOptions::default()
    }
}

// ---- event-stream wire frames (the SDK's event-stream encoding) ----

fn wire_frame(headers: &[(&str, &str)], payload: &[u8]) -> Vec<u8> {
    let mut header_bytes = Vec::new();
    for (name, value) in headers {
        header_bytes.push(name.len() as u8);
        header_bytes.extend_from_slice(name.as_bytes());
        header_bytes.push(7); // string header
        header_bytes.extend_from_slice(&(value.len() as u16).to_be_bytes());
        header_bytes.extend_from_slice(value.as_bytes());
    }
    let total = 12 + header_bytes.len() + payload.len() + 4;
    let mut frame = Vec::with_capacity(total);
    frame.extend_from_slice(&(total as u32).to_be_bytes());
    frame.extend_from_slice(&(header_bytes.len() as u32).to_be_bytes());
    frame.extend_from_slice(&crc32(&frame).to_be_bytes());
    frame.extend_from_slice(&header_bytes);
    frame.extend_from_slice(payload);
    frame.extend_from_slice(&crc32(&frame).to_be_bytes());
    frame
}

fn event_frame(event_type: &str, payload: &Value) -> Vec<u8> {
    wire_frame(
        &[
            (":message-type", "event"),
            (":event-type", event_type),
            (":content-type", "application/json"),
        ],
        payload.to_string().as_bytes(),
    )
}

/// Standard stream fixture: start, one text delta, stop, end_turn.
fn text_stream_frames() -> Vec<Vec<u8>> {
    vec![
        event_frame("messageStart", &json!({ "role": "assistant" })),
        event_frame(
            "contentBlockDelta",
            &json!({ "contentBlockIndex": 0, "delta": { "text": "hi" } }),
        ),
        event_frame("contentBlockStop", &json!({ "contentBlockIndex": 0 })),
        event_frame("messageStop", &json!({ "stopReason": "end_turn" })),
    ]
}

async fn serve_frames(server: &wiremock::MockServer, frames: &[Vec<u8>]) {
    let mut body = Vec::new();
    for frame in frames {
        body.extend_from_slice(frame);
    }
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_bytes(body)
                .insert_header("content-type", "application/vnd.amazon.eventstream")
                .insert_header("x-amzn-requestid", "req-123"),
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

/// Both hooks observe the real generation in the upstream order: onPayload
/// before the send (its replacement — not the built input — is serialized and
/// signed, mirroring the command-input replacement at lines 280-283), and
/// onResponse after the send resolves with the raw HTTP response (status and
/// the `x-amzn-requestid` header) before the stream is consumed.
#[tokio::test]
async fn callbacks_replace_the_signed_body_and_order_lifecycle_before_the_stream() {
    let _env = cleared_aws_env();
    let server = wiremock::MockServer::start().await;
    serve_frames(&server, &text_stream_frames()).await;
    let stream_model = model();
    let trace = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut callbacks = RequestCallbacks::default();
    let payload_trace = Arc::clone(&trace);
    callbacks.on_payload = Some(Arc::new(move |mut payload, hook_model| {
        assert_eq!(
            hook_model.id,
            "us.anthropic.claude-sonnet-4-5-20250929-v1:0"
        );
        assert_eq!(
            payload["modelId"],
            "us.anthropic.claude-sonnet-4-5-20250929-v1:0"
        );
        payload_trace.lock().unwrap().push("payload".into());
        payload["hooked"] = json!(true);
        Box::pin(async move { Ok(Some(payload)) })
    }));
    let response_trace = Arc::clone(&trace);
    callbacks.on_response = Some(Arc::new(move |response, _| {
        let mut trace = response_trace.lock().unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(
            response.headers.get("x-amzn-requestid").map(String::as_str),
            Some("req-123"),
            "the hook receives the raw header record"
        );
        trace.push("response".into());
        Box::pin(async { Ok(()) })
    }));

    let mut scoped_model = stream_model.clone();
    scoped_model.base_url = server.uri();
    let rx =
        BedrockConverseStream.stream(&config(), &scoped_model, &ctx(), &options_with(callbacks));
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
    assert_eq!(
        body["hooked"], true,
        "the replacement reaches the signed wire body"
    );
    assert_eq!(terminal(&events).stop_reason, StopReason::Stop);
}

/// The oracle's replacement rule (bedrock-converse-stream.ts:280-283): a hook
/// returning `undefined` (the port's `None`) keeps the built command input.
#[tokio::test]
async fn callbacks_none_preserves_the_built_command_input() {
    let _env = cleared_aws_env();
    let server = wiremock::MockServer::start().await;
    serve_frames(&server, &text_stream_frames()).await;
    let mut callbacks = RequestCallbacks::default();
    callbacks.on_payload = Some(Arc::new(|_, _| Box::pin(async { Ok(None) })));
    let mut scoped_model = model();
    scoped_model.base_url = server.uri();
    let rx =
        BedrockConverseStream.stream(&config(), &scoped_model, &ctx(), &options_with(callbacks));
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
        body["modelId"],
        json!("us.anthropic.claude-sonnet-4-5-20250929-v1:0"),
        "the built input reaches the wire unchanged"
    );
}

/// A payload-hook rejection throws into the catch block before any HTTP
/// traffic (upstream lines 280/345-356).
#[tokio::test]
async fn callbacks_payload_failure_is_pre_send() {
    let _env = cleared_aws_env();
    let server = wiremock::MockServer::start().await;
    serve_frames(&server, &text_stream_frames()).await;
    let mut callbacks = RequestCallbacks::default();
    callbacks.on_payload = Some(Arc::new(|_, _| {
        Box::pin(async { anyhow::bail!("payload refused") })
    }));
    let mut scoped_model = model();
    scoped_model.base_url = server.uri();
    let rx =
        BedrockConverseStream.stream(&config(), &scoped_model, &ctx(), &options_with(callbacks));
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
/// server saw the request, the event stream is never consumed (no `start` —
/// the middleware fires at the deserialize step, lines 510-526).
#[tokio::test]
async fn callbacks_response_failure_is_pre_stream_post_send() {
    let _env = cleared_aws_env();
    let server = wiremock::MockServer::start().await;
    serve_frames(&server, &text_stream_frames()).await;
    let mut callbacks = RequestCallbacks::default();
    callbacks.on_response = Some(Arc::new(|_, _| {
        Box::pin(async { anyhow::bail!("response refused") })
    }));
    let mut scoped_model = model();
    scoped_model.base_url = server.uri();
    let rx =
        BedrockConverseStream.stream(&config(), &scoped_model, &ctx(), &options_with(callbacks));
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
    assert!(BedrockConverseStream.supports_request_callbacks());
}
