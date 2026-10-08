//! Offline loopback tests for the process-local request callbacks
//! (azure-openai-responses.ts:113 onPayload, :130 onResponse): the seam
//! semantics pinned by `tests/fixtures/callback_seam_oracle` — a replacement
//! reaches the wire, `undefined` keeps the built body, hook rejections take
//! the pre-send (payload) / pre-start (response) error paths, and a failed
//! send never reaches onResponse.
use super::*;
use crate::ai::transcript::{normalize_context, Context};
use crate::ai::types::message::AssistantMessage;
use crate::ai::types::primitives::ModelCost;
use crate::ai::types::{Model, ModelInput};
use serde_json::json;
use std::sync::{Arc, Mutex};

const COMPLETED_SSE: &str = concat!(
    r#"data: {"type":"response.completed","response":{"id":"resp_ok","status":"completed","#,
    r#""usage":{"input_tokens":20,"output_tokens":7,"total_tokens":27}}}"#,
    "\n\n"
);

fn model(server: &wiremock::MockServer) -> Model {
    Model {
        r#type: None,
        prompt_cache: None,
        input_limits: None,
        id: "gpt-4o-mini".to_string(),
        name: "GPT-4o mini".to_string(),
        api: "azure-openai-responses".to_string(),
        provider: "azure-openai-responses".to_string(),
        base_url: server.uri(),
        reasoning: false,
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

fn ctx() -> TranscriptContext {
    normalize_context(&Context {
        system_prompt: None,
        messages: vec![],
        tools: None,
    })
}

fn config() -> ProviderConfig {
    ProviderConfig {
        // The port resolves the endpoint from env/model.baseUrl, never from
        // the ProviderConfig base_url.
        base_url: "https://unused.example.com".to_string(),
        api_key: "test-api-key".to_string(),
        max_tokens: 8192,
    }
}

/// The scoped env pins the endpoint at the loopback server; scoped entries
/// win over any process environment (upstream `getProviderEnvValue`).
fn env_with_base_url(server: &wiremock::MockServer) -> ProviderEnv {
    [
        (
            "AZURE_OPENAI_BASE_URL".to_string(),
            format!("{}/v1", server.uri()),
        ),
        ("AZURE_OPENAI_API_VERSION".to_string(), "v1".to_string()),
    ]
    .into_iter()
    .collect()
}

async fn mount(server: &wiremock::MockServer) {
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/responses"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .insert_header("x-callback", "observed")
                .set_body_string(COMPLETED_SSE),
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
/// the send resolves (status and raw headers) and before `start` — and the
/// simple path carries the callbacks identically (upstream streamSimple
/// delegates to `stream` through `buildBaseOptions`, lines 165-186).
#[tokio::test]
async fn callbacks_replace_the_wire_body_and_order_lifecycle_around_start() {
    let server = wiremock::MockServer::start().await;
    mount(&server).await;
    let stream_model = model(&server);
    let trace = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut options = StreamOptions {
        env: Some(env_with_base_url(&server)),
        ..Default::default()
    };
    let payload_trace = Arc::clone(&trace);
    options.callbacks.on_payload = Some(Arc::new(move |mut payload, hook_model| {
        assert_eq!(hook_model.id, "gpt-4o-mini");
        assert_eq!(payload["model"], "gpt-4o-mini");
        payload_trace.lock().unwrap().push("payload".into());
        payload["hooked"] = json!(true);
        Box::pin(async move { Ok(Some(payload)) })
    }));
    let response_trace = Arc::clone(&trace);
    options.callbacks.on_response = Some(Arc::new(move |response, _| {
        let mut trace = response_trace.lock().unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(
            response.headers.get("x-callback").map(String::as_str),
            Some("observed")
        );
        trace.push("response".into());
        Box::pin(async { Ok(()) })
    }));

    let api = AzureOpenAiResponses;
    let rx = api.stream(&config(), &stream_model, &ctx(), &options);
    let events = tokio::time::timeout(std::time::Duration::from_secs(5), collect(rx))
        .await
        .expect("bounded local stream");
    assert_eq!(types(&events), ["start", "done"]);
    assert_eq!(
        *trace.lock().unwrap(),
        ["payload".to_string(), "response".to_string()],
        "onResponse precedes start and onPayload precedes the send"
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["hooked"], true, "the replacement reaches the wire");
    assert_eq!(body["stream"], true);

    // The simple path carries the same hooks to the same seams.
    let server = wiremock::MockServer::start().await;
    mount(&server).await;
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let payload_seen = Arc::clone(&seen);
    let mut simple = SimpleStreamOptions {
        stream: StreamOptions {
            env: Some(env_with_base_url(&server)),
            ..Default::default()
        },
        ..SimpleStreamOptions::default()
    };
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
    let rx = api.stream_simple(&config(), &model(&server), &ctx(), &simple);
    let events = tokio::time::timeout(std::time::Duration::from_secs(5), collect(rx))
        .await
        .expect("bounded local stream");
    assert_eq!(types(&events), ["start", "done"]);
    assert_eq!(
        *seen.lock().unwrap(),
        ["payload".to_string(), "response".to_string()]
    );
    let body: Value =
        serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
    assert_eq!(body["hooked"], true);
}

/// The oracle's replacement rule (azure-openai-responses.ts:113-116): a hook
/// returning `undefined` (the port's `None`) keeps the built body.
#[tokio::test]
async fn callbacks_none_preserves_the_built_body() {
    let server = wiremock::MockServer::start().await;
    mount(&server).await;
    let mut options = StreamOptions {
        env: Some(env_with_base_url(&server)),
        ..Default::default()
    };
    options.callbacks.on_payload = Some(Arc::new(|_, _| Box::pin(async { Ok(None) })));
    let rx = AzureOpenAiResponses.stream(&config(), &model(&server), &ctx(), &options);
    let events = tokio::time::timeout(std::time::Duration::from_secs(5), collect(rx))
        .await
        .expect("bounded local stream");
    assert_eq!(types(&events), ["start", "done"]);
    let body: Value =
        serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
    assert_eq!(body["hooked"], Value::Null);
    assert_eq!(body["model"], "gpt-4o-mini");
}

/// A payload-hook rejection throws into the catch block before any HTTP
/// traffic (upstream lines 113/148-159).
#[tokio::test]
async fn callbacks_payload_failure_is_pre_send() {
    let server = wiremock::MockServer::start().await;
    mount(&server).await;
    let mut options = StreamOptions {
        env: Some(env_with_base_url(&server)),
        ..Default::default()
    };
    options.callbacks.on_payload = Some(Arc::new(|_, _| {
        Box::pin(async { anyhow::bail!("payload refused") })
    }));
    let rx = AzureOpenAiResponses.stream(&config(), &model(&server), &ctx(), &options);
    let events = tokio::time::timeout(std::time::Duration::from_secs(5), collect(rx))
        .await
        .expect("bounded local stream");
    assert_eq!(types(&events), ["error"]);
    assert_eq!(terminal(&events).stop_reason, StopReason::Error);
    assert_eq!(
        terminal(&events).error_message.as_deref(),
        Some("payload refused")
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

/// A response-hook rejection takes the same catch path after the send: the
/// server saw the request, `start` never fires (upstream line 130 precedes
/// the `start` push at line 131).
#[tokio::test]
async fn callbacks_response_failure_is_pre_start_post_send() {
    let server = wiremock::MockServer::start().await;
    mount(&server).await;
    let mut options = StreamOptions {
        env: Some(env_with_base_url(&server)),
        ..Default::default()
    };
    options.callbacks.on_response = Some(Arc::new(|_, _| {
        Box::pin(async { anyhow::bail!("response refused") })
    }));
    let rx = AzureOpenAiResponses.stream(&config(), &model(&server), &ctx(), &options);
    let events = tokio::time::timeout(std::time::Duration::from_secs(5), collect(rx))
        .await
        .expect("bounded local stream");
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
    assert!(AzureOpenAiResponses.supports_request_callbacks());
}
