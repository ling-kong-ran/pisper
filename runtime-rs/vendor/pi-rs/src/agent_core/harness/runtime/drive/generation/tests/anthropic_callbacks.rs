//! Exercise callback-bearing requests through real Models, Lane, and generation.
use super::*;
use crate::agent_core::harness::hooks::PayloadHookResult;
use crate::ai::api::anthropic::AnthropicMessages;
use crate::ai::models::provider::{create_provider, ApiImpls, CreateProviderOptions, Provider};
use crate::ai::types::{Model, StopReason};
use std::collections::BTreeMap;
use wiremock::{Mock, MockServer, ResponseTemplate};

const SSE: &str = "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
fn provider(base_url: &str) -> (Model, Arc<dyn Provider>) {
    let faux = faux_provider(FauxProviderOptions::default());
    let mut model = faux.provider.get_models().unwrap().remove(0);
    model.id = "local-anthropic".into();
    model.provider = "local-anthropic".into();
    model.api = "anthropic-messages".into();
    model.base_url = base_url.into();
    let provider = create_provider(CreateProviderOptions {
        filter_all_models: None,
        images: crate::ai::models::provider::ImagesImpls::new(),
        classifiers: crate::ai::models::provider::ClassifiersImpls::new(),
        id: model.provider.clone(),
        name: None,
        base_url: None,
        headers: None,
        auth: faux.provider.auth().clone(),
        models: vec![crate::ai::types::AnyModel::Chat(model.clone())],
        fetch_models: None,
        filter_models: None,
        api: ApiImpls::Single(Arc::new(AnthropicMessages)),
    });
    (model, provider)
}
async fn fixture(base_url: &str) -> Fixture {
    let (_, provider) = provider(base_url);
    let f = Fixture::with_provider("review", Some(provider)).await;
    let mut ready = f.state();
    configured(&mut ready).model = LaneModel {
        provider: "local-anthropic".into(),
        model_id: "local-anthropic".into(),
    };
    // The checkpoint has already captured stream options durably. Configure
    // that generation context, rather than changing RuntimeConfig too late.
    let OperationPhase::AssistantReady {
        generation_context, ..
    } = &mut ready.phase
    else {
        panic!("fixture must be assistant-ready");
    };
    generation_context.stream_options.headers = Some(BTreeMap::from([(
        "Authorization".into(),
        "Bearer local-test-only".into(),
    )]));
    f.set_state(ready).await;
    f
}

#[tokio::test]
async fn generation_anthropic_hooks_patch_wire_and_publish_a_durable_assistant() {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/messages"))
        .and(wiremock::matchers::query_param("beta", "true"))
        .and(wiremock::matchers::header("anthropic-beta", "patched-beta"))
        .and(wiremock::matchers::body_json(
            json!({"patchedByHarness":true,"stream":true}),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(SSE.as_bytes(), "text/event-stream")
                .insert_header("x-harness-response", "local"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let f = fixture(&server.uri()).await;
    let trace = Arc::new(Mutex::new(Vec::new()));
    let payload_trace = Arc::clone(&trace);
    let response_trace = Arc::clone(&trace);
    f.lane.hooks().on(HookName::BeforePayload,Arc::new(move |invocation,context| {
        assert_eq!(invocation.lane,"review");assert!(context.abort_signal().is_some());
        let HookEvent::BeforePayload(event)=invocation.event else {panic!("payload")};
        assert_eq!(event.model.id,"local-anthropic");assert_eq!(event.payload["model"],"local-anthropic");assert!(!event.payload["messages"].as_array().unwrap().is_empty());
        payload_trace.lock().unwrap().push("payload");
        Box::pin(async {Ok(HookResult::BeforePayload(Some(PayloadHookResult {payload:json!({"patchedByHarness":true,"stream":false,"betas":["patched-beta"]})})))})
    }),None).unwrap();
    f.lane
        .hooks()
        .on(
            HookName::AfterResponse,
            Arc::new(move |invocation, _| {
                let HookEvent::AfterResponse(event) = invocation.event else {
                    panic!("response")
                };
                assert_eq!(event.status, Some(200));
                let headers = event.headers.unwrap();
                assert_eq!(headers["x-harness-response"], "local");
                assert_eq!(headers["content-type"], "text/event-stream");
                assert_eq!(event.message.stop_reason, StopReason::Stop);
                response_trace.lock().unwrap().push("response");
                Box::pin(async { Ok(HookResult::AfterResponse(None)) })
            }),
            None,
        )
        .unwrap();
    assert!(matches!(
        run_generation(&f.lane, &f.drive, &f.state()).await.unwrap(),
        ProcedureResult::Continue
    ));
    assert_eq!(*trace.lock().unwrap(), ["payload", "response"]);
    assert!(f.state().scope.latest_assistant_entry_id.is_some());
    assert!(matches!(f.state().phase, OperationPhase::Checkpoint { .. }));
    let events = f.events.lock().unwrap();
    assert!(events
        .iter()
        .any(|e| matches!(e, HarnessEvent::EntryAdded { .. })));
    assert!(events.iter().any(|e| matches!(e,HarnessEvent::MessageStart {lane,run_id:Some(run),..} if lane=="review" && run==f.drive.operation_id())));
    assert!(events.iter().any(|e| matches!(e,HarnessEvent::MessageEnd {message:AgentMessage::Assistant(message),..} if message.stop_reason==StopReason::Stop)));
    assert_eq!(
        f.faux.state().lock().unwrap().call_count,
        0,
        "the real adapter must run, not Faux"
    );
}

#[tokio::test]
async fn generation_anthropic_http_failure_settles_without_success_metadata() {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_string("denied"))
        .expect(1)
        .mount(&server)
        .await;
    let f = fixture(&server.uri()).await;
    let after = Arc::new(Mutex::new(0));
    let called = Arc::clone(&after);
    f.lane
        .hooks()
        .on(
            HookName::AfterResponse,
            Arc::new(move |invocation, _| {
                let HookEvent::AfterResponse(event) = invocation.event else {
                    panic!("settlement")
                };
                assert_eq!(event.status, None);
                assert_eq!(event.headers, None);
                assert_eq!(event.message.stop_reason, StopReason::Error);
                *called.lock().unwrap() += 1;
                Box::pin(async { Ok(HookResult::AfterResponse(None)) })
            }),
            None,
        )
        .unwrap();
    let prepared = f.prepared().await;
    let intent = f.intent(&prepared).await;
    let message = perform_generation(&f.lane, &f.drive, &intent, &prepared)
        .await
        .unwrap();
    assert_eq!(message.stop_reason, StopReason::Error);
    assert_eq!(message.error_message.as_deref(), Some("401: denied"));
    assert_eq!(*after.lock().unwrap(), 1);
    assert!(f.events.lock().unwrap().iter().any(|e|matches!(e,HarnessEvent::MessageEnd {message:AgentMessage::Assistant(message),..} if message.stop_reason==StopReason::Error)));
}

#[tokio::test]
async fn models_normal_anthropic_route_retains_callbacks_and_allows_capability() {
    use crate::ai::models::ModelsApiStreamOptions;
    use crate::ai::types::options::StreamOptions;
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::body_json(
            json!({"fromModels":true,"stream":true}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_raw(SSE.as_bytes(), "text/event-stream"))
        .expect(1)
        .mount(&server)
        .await;
    let (model, provider) = provider(&server.uri());
    let mut models = create_models(CreateModelsOptions::default());
    models.set_provider(provider);
    let response_count = Arc::new(Mutex::new(0));
    let called = Arc::clone(&response_count);
    let mut stream = StreamOptions {
        headers: Some(BTreeMap::from([(
            "Authorization".into(),
            Some("Bearer local-test-only".into()),
        )])),
        ..Default::default()
    };
    stream.callbacks.on_payload = Some(Arc::new(|_, _| {
        Box::pin(async { Ok(Some(json!({"fromModels":true,"stream":false}))) })
    }));
    stream.callbacks.on_response = Some(Arc::new(move |metadata, _| {
        assert_eq!(metadata.status, 200);
        *called.lock().unwrap() += 1;
        Box::pin(async { Ok(()) })
    }));
    let message = models
        .complete(
            &model,
            &crate::ai::transcript::Context {
                system_prompt: None,
                messages: vec![],
                tools: None,
            },
            Some(ModelsApiStreamOptions {
                stream,
                transform_headers: None,
            }),
        )
        .await;
    assert_eq!(message.stop_reason, StopReason::Stop);
    assert_eq!(*response_count.lock().unwrap(), 1);
}
