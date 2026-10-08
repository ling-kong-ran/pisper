//! Independent integration tests over real Lane/session, hooks, and Models.
//! No test may silently return when an expected operation is absent.
use super::*;
use crate::agent_core::harness::context::background_context;
use crate::agent_core::harness::execution::effect_gate::GateRejection;
use crate::agent_core::harness::hooks::{
    BeforeRequestHookResult, HookName, HookRegistry, MessageHookResult, TransformContextPatch,
};
use crate::agent_core::harness::runtime::drive::checkpoint::{run_checkpoint, start_run};
use crate::agent_core::harness::runtime::drive_pass::{DriveOptions, DriveOutcome};
use crate::agent_core::harness::runtime::durable::{Continuation, RetryWait};
use crate::agent_core::harness::runtime::lane::{EmitBatch, OperationRequest, RuntimeConfig};
use crate::agent_core::harness::runtime::restore::restore_lane;
use crate::agent_core::harness::session::{
    self, set_value, Control, LaneConfiguration, LaneModel, MemoryStorage, MemoryStorageOptions,
    SessionMetadata, StorageBackedSession,
};
use crate::agent_core::harness::types::AgentHarnessStreamOptionsPatch;
use crate::agent_core::types::{QueueMode, ThinkingLevel};
use crate::ai::models::faux::{
    faux_assistant_message, faux_provider, FauxProviderHandle, FauxProviderOptions,
    FauxResponseStep,
};
use crate::ai::models::{create_models, CreateModelsOptions};
use crate::ai::types::message::{Message, StringOrBlocks, TextOrImageBlock, UserMessage};
use serde_json::json;
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;

struct Fixture {
    lane: Arc<Lane>,
    drive: Arc<Drive>,
    faux: FauxProviderHandle,
    config: Arc<Mutex<RuntimeConfig>>,
    events: Arc<Mutex<Vec<HarnessEvent>>>,
    fail_event: Arc<Mutex<Option<&'static str>>>,
}

impl Fixture {
    async fn new(name: &str) -> Self {
        Self::with_provider(name, None).await
    }
    async fn with_provider(
        name: &str,
        extra: Option<Arc<dyn crate::ai::models::provider::Provider>>,
    ) -> Self {
        let config = Arc::new(Mutex::new(RuntimeConfig::default()));
        let stored_config = Arc::clone(&config);
        let storage = MemoryStorage::new(MemoryStorageOptions::default());
        let session = Arc::new(StorageBackedSession::new(
            SessionMetadata {
                id: "generation-session".into(),
                created_at: 1,
                storage_version: 1,
                ..Default::default()
            },
            Arc::new(storage),
        ));
        let writes = vec![
            set_value(&session::branch_tip(name), json!(null)),
            set_value(
                &session::lane_config(name),
                session::lane_configuration_value(&LaneConfiguration {
                    model: LaneModel {
                        provider: "faux".into(),
                        model_id: "faux-1".into(),
                    },
                    thinking_level: ThinkingLevel::Off,
                    active_tool_names: Vec::new(),
                }),
            ),
            set_value(
                &session::lane_state(name),
                json!({"currentOperationId":null,"lastOperationId":null,"inbox":[]}),
            ),
        ];
        session
            .mutate(
                move |reader, context| {
                    let writes = writes.clone();
                    Box::pin(async move { reader.commit(writes, context).await.map(|_| ()) })
                },
                background_context(),
            )
            .await
            .unwrap();
        let faux = faux_provider(FauxProviderOptions {
            api: Some("faux-generation".into()),
            ..Default::default()
        });
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(Arc::clone(&faux.provider));
        if let Some(provider) = extra {
            models.set_provider(provider);
        }
        let state = restore_lane(session.as_ref(), name, background_context())
            .await
            .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&events);
        let fail_event = Arc::new(Mutex::new(None));
        let failure = Arc::clone(&fail_event);
        let emit: EmitBatch = Arc::new(move |batch, _| {
            let recorded = Arc::clone(&recorded);
            let failure = Arc::clone(&failure);
            Box::pin(async move {
                for event in &batch {
                    let kind = match event {
                        HarnessEvent::MessageStart { .. } => "start",
                        HarnessEvent::MessageUpdate { .. } => "update",
                        HarnessEvent::MessageEnd { .. } => "end",
                        _ => "other",
                    };
                    if *failure.lock().unwrap() == Some(kind) {
                        anyhow::bail!("observer {kind} failure");
                    }
                }
                recorded.lock().unwrap().extend(batch);
                Ok(())
            })
        });
        let hooks = HookRegistry::new(Arc::new(|_, _, _, _| Box::pin(async {})));
        let lane = Lane::new(
            name,
            session,
            models,
            hooks,
            state,
            Arc::new(|e| e),
            emit,
            Arc::new(move || stored_config.lock().unwrap().clone()),
        );
        let admission = lane
            .accept(
                &OperationRequest::Prompt {
                    prompt: "hello generation".into(),
                },
                background_context(),
            )
            .await
            .unwrap()
            .unwrap();
        let drive = Arc::new(Drive::new(
            &DriveOptions {
                operation_id: admission.operation_id,
                wait_for_retry: None,
                poll_deferred: None,
            },
            background_context(),
        ));
        let starting = lane
            .state()
            .operation
            .expect("admission creates operation")
            .state;
        assert!(matches!(
            start_run(&lane, &drive, &starting).await.unwrap(),
            super::super::publish::PublishOutcome::Continue
        ));
        let checkpoint = lane.state().operation.unwrap().state;
        assert!(matches!(
            run_checkpoint(&lane, &drive, &checkpoint).await.unwrap(),
            super::super::publish::PublishOutcome::Continue
        ));
        assert!(matches!(
            lane.state().operation.unwrap().state.phase,
            OperationPhase::AssistantReady { .. }
        ));
        events.lock().unwrap().clear();
        Self {
            lane,
            drive,
            faux,
            config,
            events,
            fail_event,
        }
    }
    fn state(&self) -> OperationState {
        self.lane
            .state()
            .operation
            .expect("operation remains present")
            .state
    }
    async fn set_state(&self, state: OperationState) {
        self.lane
            .settle_operation(
                move |_, _, _, _| {
                    let state = state.clone();
                    Box::pin(async move {
                        Ok(OperationCommand::Commit {
                            writes: vec![],
                            operation_state: state,
                            lane: None,
                            materialize: Box::new(|_| ()),
                            events: None,
                        })
                    })
                },
                background_context(),
            )
            .await
            .unwrap();
    }
    async fn prepared(&self) -> PreparedGeneration {
        match prepare_generation(&self.lane, &self.drive, &self.state())
            .await
            .unwrap()
        {
            GenerationPreparation::Ready(p) => p,
            _ => panic!("must prepare a generation"),
        }
    }
    async fn intent(&self, prepared: &PreparedGeneration) -> OperationState {
        match publish_generation_intent(&self.lane, &self.drive, &self.state(), prepared)
            .await
            .unwrap()
        {
            ContinueOperationResult::Result { value } => value,
            _ => panic!("intent unexpectedly cancelled"),
        }
    }
    async fn frames(&self, response_id: &str) -> Vec<serde_json::Value> {
        let run_id = self.drive.operation_id().to_owned();
        let response_id = response_id.to_owned();
        self.lane.command(move |_,reader| { let run_id=run_id.clone(); let response_id=response_id.clone(); Box::pin(async move {
            Ok(crate::agent_core::harness::runtime::lane::LaneCommand::Return {
                result:crate::agent_core::harness::runtime::progress::read_assistant_frames(reader,&run_id,&response_id,background_context()).await?
            })
        })},background_context()).await.unwrap()
    }
    fn queue(&self, text: &str) {
        self.faux
            .append_responses(vec![faux_assistant_message(text, Default::default()).into()]);
    }
    async fn retry(&self, not_before: i64) -> OperationState {
        let mut state = self.state();
        let OperationPhase::AssistantReady {
            generation_context, ..
        } = state.phase
        else {
            panic!("ready")
        };
        state.phase = OperationPhase::AssistantRetryWait {
            generation_context,
            retry: RetryWait {
                next_attempt: 2,
                not_before,
                error_message: "retry me".into(),
            },
        };
        self.set_state(state.clone()).await;
        state
    }
}
fn user(text: &str) -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: StringOrBlocks::Text(text.into()),
        timestamp: 1,
    })
}
fn configured(state: &mut OperationState) -> &mut session::LaneConfiguration {
    let OperationPhase::AssistantReady {
        generation_context, ..
    } = &mut state.phase
    else {
        panic!("ready")
    };
    &mut generation_context.configuration
}

#[tokio::test]
async fn unavailable_model_and_tools_preserve_exact_error_shapes_and_order() {
    let f = Fixture::new("main").await;
    let mut ready = f.state();
    configured(&mut ready).model.model_id = "missing".into();
    match prepare_generation(&f.lane, &f.drive, &ready).await.unwrap() {
        GenerationPreparation::ConfigurationFailure { error } => assert_eq!(
            serde_json::to_value(error).unwrap(),
            json!({"code":"model_unavailable","message":"The configured model is unavailable in this process","details":{"provider":"faux","modelId":"missing"}})
        ),
        _ => panic!("expected missing model"),
    }
    let mut ready = f.state();
    configured(&mut ready).active_tool_names = vec!["b".into(), "a".into(), "b".into()];
    match prepare_generation(&f.lane, &f.drive, &ready).await.unwrap() {
        GenerationPreparation::ConfigurationFailure { error } => {
            assert_eq!(error.details, Some(json!({"tools":["b","a","b"]})))
        }
        _ => panic!("expected missing tools"),
    }
    assert_eq!(f.faux.state().lock().unwrap().call_count, 0);
    assert_eq!(f.state(), f.lane.state().operation.unwrap().state);
}

#[tokio::test]
async fn preparation_selects_tools_in_active_order_and_keeps_last_duplicate() {
    let f = Fixture::new("main").await;
    let make = |name: &str, description: &str| {
        serde_json::from_value(
            json!({"name":name,"description":description,"parameters":{"type":"object"}}),
        )
        .unwrap()
    };
    f.config.lock().unwrap().tools = vec![make("a", "old"), make("b", "middle"), make("a", "last")];
    f.config.lock().unwrap().system_prompt = Some("system".into());
    let mut ready = f.state();
    configured(&mut ready).active_tool_names = vec!["b".into(), "a".into(), "b".into()];
    f.set_state(ready).await;
    let p = f.prepared().await;
    assert_eq!(
        p.tools
            .iter()
            .map(|t| t.description.as_str())
            .collect::<Vec<_>>(),
        ["middle", "last", "middle"]
    );
    assert_eq!(p.system_prompt, "system");
    // Upstream lane.ts:505-527 always builds the prompt message with the
    // block-array content form ([text?, ...images]) — asserted via the block
    // variant, not the StringOrBlocks::Text short form.
    assert!(p.messages.iter().any(|m| matches!(
        m,
        AgentMessage::User(UserMessage {
            content: StringOrBlocks::Blocks(blocks),
            ..
        }) if blocks.iter().any(|b| matches!(b, TextOrImageBlock::Text(t) if t.text == "hello generation"))
    )));
}

#[tokio::test]
async fn before_request_patch_is_request_local_and_has_drive_identity() {
    let f = Fixture::new("review").await;
    let run = f.drive.operation_id().to_owned();
    f.lane
        .hooks()
        .on(
            HookName::BeforeRequest,
            Arc::new(move |invocation, context| {
                assert_eq!(invocation.lane, "review");
                assert_eq!(invocation.run_id, run);
                assert!(context.abort_signal().is_some());
                let HookEvent::BeforeRequest(event) = invocation.event else {
                    panic!("before request")
                };
                assert_eq!(event.attempt, 1);
                Box::pin(async {
                    Ok(HookResult::BeforeRequest(Some(BeforeRequestHookResult {
                        stream_options: serde_json::from_value::<AgentHarnessStreamOptionsPatch>(
                            json!({"timeoutMs":4321,"headers":{"x-hook":"yes"}}),
                        )
                        .unwrap(),
                    })))
                })
            }),
            None,
        )
        .unwrap();
    let before = f.state();
    let p = f.prepared().await;
    assert_eq!(p.stream_options.timeout_ms, Some(4321));
    assert_eq!(p.stream_options.headers.unwrap()["x-hook"], "yes");
    assert_eq!(f.state(), before);
}

#[tokio::test]
async fn cancelled_preparation_and_intent_never_start_provider_or_turn() {
    let f = Fixture::new("main").await;
    let prepared = f.prepared().await;
    let ready = f.state();
    let mut current = ready.clone();
    current.scope.control = Control::CancelRequested { requested_at: 3 };
    f.set_state(current.clone()).await;
    assert!(matches!(
        prepare_generation(&f.lane, &f.drive, &ready).await.unwrap(),
        GenerationPreparation::CancelRequested
    ));
    assert!(matches!(
        publish_generation_intent(&f.lane, &f.drive, &ready, &prepared)
            .await
            .unwrap(),
        ContinueOperationResult::CancelRequested
    ));
    assert_eq!(f.state(), current);
    assert!(f.events.lock().unwrap().is_empty());
    assert_eq!(f.faux.state().lock().unwrap().call_count, 0);
}

#[tokio::test]
async fn intent_keeps_current_scope_and_reserves_distinct_uuidv7_ids() {
    let f = Fixture::new("review").await;
    let p = f.prepared().await;
    let ready = f.state();
    let mut current = ready.clone();
    current.scope.settings.steering_mode = QueueMode::All;
    current.scope.latest_assistant_entry_id = Some("latest-from-current".into());
    f.set_state(current.clone()).await;
    let ContinueOperationResult::Result { value: intent } =
        publish_generation_intent(&f.lane, &f.drive, &ready, &p)
            .await
            .unwrap()
    else {
        panic!("intent")
    };
    assert_eq!(intent.scope, current.scope);
    assert_eq!(intent, f.state());
    let OperationPhase::AssistantEffectPending {
        response_entry_id,
        usage_id,
        intended_output_limit,
        context_window,
        ..
    } = &intent.phase
    else {
        panic!("pending")
    };
    assert_ne!(response_entry_id, usage_id);
    assert_eq!(response_entry_id.as_bytes()[14], b'7');
    assert_eq!(usage_id.as_bytes()[14], b'7');
    assert_eq!(
        super::super::response::uuid_v7_timestamp(response_entry_id).unwrap(),
        super::super::response::uuid_v7_timestamp(usage_id).unwrap()
    );
    assert_eq!(*intended_output_limit, p.model.max_tokens);
    assert_eq!(*context_window, p.model.context_window);
    assert!(
        matches!(f.events.lock().unwrap().as_slice(),[HarnessEvent::TurnStart {lane,run_id,recovery:false,..}] if lane=="review" && run_id==f.drive.operation_id())
    );
    assert_eq!(f.faux.state().lock().unwrap().call_count, 0);
}

#[tokio::test]
async fn retry_intent_does_not_emit_a_second_turn_start() {
    let f = Fixture::new("main").await;
    let mut ready = f.state();
    let OperationPhase::AssistantReady { next_attempt, .. } = &mut ready.phase else {
        panic!("ready")
    };
    *next_attempt = 2;
    f.set_state(ready).await;
    let p = f.prepared().await;
    f.intent(&p).await;
    assert!(f.events.lock().unwrap().is_empty());
}

#[tokio::test]
async fn future_retry_without_wait_is_read_only_and_reports_deadline() {
    let f = Fixture::new("main").await;
    let deadline = crate::ai::now_ms() + 60_000;
    let retry = f.retry(deadline).await;
    assert_eq!(
        run_generation(&f.lane, &f.drive, &retry).await.unwrap(),
        ProcedureResult::Waiting {
            outcome: DriveOutcome::Waiting {
                operation_id: f.drive.operation_id().into(),
                reason: WaitingReason::Retry {
                    not_before: deadline
                }
            }
        }
    );
    assert_eq!(f.state(), retry);
    assert!(f.events.lock().unwrap().is_empty());
}

#[tokio::test]
async fn elapsed_retry_preserves_scope_and_emits_the_actual_run_id() {
    let f = Fixture::new("review").await;
    let retry = f.retry(0).await;
    assert!(matches!(
        run_retry_wait(&f.lane, &f.drive, &retry).await.unwrap(),
        ProcedureResult::Continue
    ));
    assert_eq!(f.state().scope, retry.scope);
    assert!(matches!(
        f.state().phase,
        OperationPhase::AssistantReady {
            next_attempt: 2,
            ..
        }
    ));
    assert!(
        matches!(f.events.lock().unwrap().as_slice(),[HarnessEvent::RetryStart {lane,run_id,attempt:2,..}] if lane=="review" && run_id==f.drive.operation_id())
    );
}

#[tokio::test]
async fn closed_gate_refuses_wait_before_starting_timer() {
    let f = Fixture::new("main").await;
    let retry = f.retry(crate::ai::now_ms() + 60_000).await;
    let drive = Drive::new(
        &DriveOptions {
            operation_id: f.drive.operation_id().into(),
            wait_for_retry: Some(true),
            poll_deferred: None,
        },
        background_context(),
    );
    drive.close_gate(Arc::new(std::io::Error::other("closed-before-wait")));
    let error = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        run_retry_wait(&f.lane, &drive, &retry),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(matches!(
        error.downcast_ref::<GateRejection>(),
        Some(GateRejection::Closed(_))
    ));
    assert_eq!(f.state(), retry);
}

#[tokio::test]
async fn cancellation_during_retry_wait_keeps_typed_refusal_and_durable_phase() {
    let f = Fixture::new("main").await;
    let retry = f.retry(crate::ai::now_ms() + 60_000).await;
    let drive = Arc::new(Drive::new(
        &DriveOptions {
            operation_id: f.drive.operation_id().into(),
            wait_for_retry: Some(true),
            poll_deferred: None,
        },
        background_context(),
    ));
    let aborter = drive.clone();
    let trigger = tokio::spawn(async move {
        tokio::task::yield_now().await;
        aborter.begin_abort(CancellationToken::new());
        aborter.signal_abort();
    });
    let error = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        run_retry_wait(&f.lane, &drive, &retry),
    )
    .await
    .unwrap()
    .unwrap_err();
    trigger.await.unwrap();
    assert!(matches!(
        error.downcast_ref::<GateRejection>(),
        Some(GateRejection::AbortRequested(_))
    ));
    assert_eq!(f.state(), retry);
}

#[tokio::test]
async fn real_stream_uses_prepared_converter_and_runs_transform_and_settlement_hooks() {
    let f = Fixture::new("review").await;
    let converted = Arc::new(Mutex::new(Vec::new()));
    let converted_out = converted.clone();
    f.config.lock().unwrap().to_provider_messages = Some(Arc::new(move |messages, _| {
        *converted_out.lock().unwrap() = messages;
        Box::pin(async {
            vec![Message::User(UserMessage {
                content: StringOrBlocks::Text("converted".into()),
                timestamp: 1,
            })]
        })
    }));
    f.lane
        .hooks()
        .on(
            HookName::TransformContext,
            Arc::new(|_, _| {
                Box::pin(async {
                    Ok(HookResult::TransformContext(Some(TransformContextPatch {
                        messages: Some(vec![user("transformed")]),
                        system_prompt: Some("hook system".into()),
                    })))
                })
            }),
            None,
        )
        .unwrap();
    let hook_seen = Arc::new(Mutex::new(false));
    let hook_out = hook_seen.clone();
    let run = f.drive.operation_id().to_owned();
    f.lane
        .hooks()
        .on(
            HookName::AfterResponse,
            Arc::new(move |invocation, _| {
                assert_eq!(invocation.lane, "review");
                assert_eq!(invocation.run_id, run);
                let HookEvent::AfterResponse(event) = invocation.event else {
                    panic!("after response")
                };
                assert_eq!(event.status, Some(200));
                assert_eq!(event.headers, Some(Default::default()));
                *hook_out.lock().unwrap() = true;
                let mut message = event.message;
                message.content = faux_assistant_message("patched", Default::default()).content;
                Box::pin(async move {
                    Ok(HookResult::AfterResponse(Some(MessageHookResult {
                        message,
                    })))
                })
            }),
            None,
        )
        .unwrap();
    let p = f.prepared().await;
    let intent = f.intent(&p).await;
    f.config.lock().unwrap().to_provider_messages = Some(Arc::new(|_, _| {
        panic!("converter must be captured at preparation")
    }));
    f.faux.set_responses(vec![FauxResponseStep::Factory(Arc::new(|args| {
        assert!(args.context.0.iter().any(|message| matches!(message, crate::ai::types::Message::System(system) if crate::ai::transcript::get_system_message_text(system) == "hook system")));
        assert_eq!(args.options.unwrap().stream.session_id.as_deref(),Some("generation-session:review"));
        Box::pin(async {Ok(faux_assistant_message("raw",Default::default()))})
    }))]);
    let message = perform_generation(&f.lane, &f.drive, &intent, &p)
        .await
        .unwrap();
    assert_eq!(
        message.content,
        faux_assistant_message("patched", Default::default()).content
    );
    assert_eq!(*converted.lock().unwrap(), vec![user("transformed")]);
    assert!(*hook_seen.lock().unwrap());
    let events = f.events.lock().unwrap();
    assert!(events.iter().any(|e| matches!(e,HarnessEvent::MessageStart {lane,run_id:Some(run_id),recovery:false,..} if lane=="review" && run_id==f.drive.operation_id())));
    assert!(events
        .iter()
        .any(|e| matches!(e,HarnessEvent::MessageUpdate {lane,..} if lane=="review")));
    assert!(
        matches!(events.last(),Some(HarnessEvent::MessageEnd {lane,message:AgentMessage::Assistant(ended),..}) if lane=="review" && ended==&message)
    );
}

#[tokio::test]
async fn closed_transform_gate_does_not_start_provider() {
    let f = Fixture::new("main").await;
    let p = f.prepared().await;
    let intent = f.intent(&p).await;
    f.drive
        .close_gate(Arc::new(std::io::Error::other("closed-transform")));
    f.queue("never");
    let error = perform_generation(&f.lane, &f.drive, &intent, &p)
        .await
        .unwrap_err();
    assert!(matches!(
        error.downcast_ref::<GateRejection>(),
        Some(GateRejection::Closed(_))
    ));
    assert_eq!(f.faux.state().lock().unwrap().call_count, 0);
}

#[tokio::test]
async fn request_gate_checks_again_after_converter_returns() {
    let f = Fixture::new("main").await;
    let drive = f.drive.clone();
    f.config.lock().unwrap().to_provider_messages = Some(Arc::new(move |_, _| {
        drive.close_gate(Arc::new(std::io::Error::other("closed-converter")));
        Box::pin(async { vec![] })
    }));
    let p = f.prepared().await;
    let intent = f.intent(&p).await;
    let error = perform_generation(&f.lane, &f.drive, &intent, &p)
        .await
        .unwrap_err();
    assert!(matches!(
        error.downcast_ref::<GateRejection>(),
        Some(GateRejection::Closed(_))
    ));
    assert_eq!(f.faux.state().lock().unwrap().call_count, 0);
}

#[tokio::test]
async fn observer_failures_propagate_for_start_update_and_end() {
    for kind in ["start", "update", "end"] {
        let f = Fixture::new("main").await;
        let p = f.prepared().await;
        let intent = f.intent(&p).await;
        f.queue("observer failure response");
        *f.fail_event.lock().unwrap() = Some(kind);
        let error = perform_generation(&f.lane, &f.drive, &intent, &p)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("observer {kind} failure")),
            "{error}"
        );
        let OperationPhase::AssistantEffectPending {
            response_entry_id, ..
        } = intent.phase
        else {
            panic!("intent")
        };
        let frames = f.frames(&response_entry_id).await;
        assert!(
            !frames.is_empty(),
            "finally must drain even when {kind} fails"
        );
        assert_eq!(frames[0]["type"], "start");
    }
}

#[tokio::test]
async fn run_generation_executes_and_publishes_a_real_assistant_response() {
    let f = Fixture::new("main").await;
    f.queue("end to end");
    assert!(matches!(
        run_generation(&f.lane, &f.drive, &f.state()).await.unwrap(),
        ProcedureResult::Continue
    ));
    assert!(matches!(
        f.state().phase,
        OperationPhase::Checkpoint {
            checkpoint: super::super::super::durable::CheckpointData {
                continuation: Continuation::MayFinish { .. },
                ..
            }
        }
    ));
    assert!(f.state().scope.latest_assistant_entry_id.is_some());
    assert_eq!(f.faux.state().lock().unwrap().call_count, 1);
    assert!(f
        .events
        .lock()
        .unwrap()
        .iter()
        .any(|e| matches!(e, HarnessEvent::EntryAdded { .. })));
}

#[tokio::test]
async fn run_generation_configuration_failure_settles_without_streaming() {
    let f = Fixture::new("main").await;
    let mut ready = f.state();
    configured(&mut ready).model.model_id = "missing".into();
    f.set_state(ready.clone()).await;
    let ProcedureResult::Settled { outcome } =
        run_generation(&f.lane, &f.drive, &ready).await.unwrap()
    else {
        panic!("settled")
    };
    assert_eq!(outcome.status, session::TerminalStatus::Failed);
    assert_eq!(outcome.error.unwrap().code, "model_unavailable");
    assert!(f.lane.state().operation.is_none());
    assert_eq!(f.faux.state().lock().unwrap().call_count, 0);
}

#[tokio::test]
async fn invalid_phases_return_errors_instead_of_panicking_or_creating_intents() {
    let f = Fixture::new("main").await;
    let p = f.prepared().await;
    let mut state = f.state();
    state.phase = OperationPhase::Starting;
    assert!(prepare_generation(&f.lane, &f.drive, &state).await.is_err());
    assert!(publish_generation_intent(&f.lane, &f.drive, &state, &p)
        .await
        .is_err());
    assert!(perform_generation(&f.lane, &f.drive, &state, &p)
        .await
        .is_err());
    assert!(run_retry_wait(&f.lane, &f.drive, &state).await.is_err());
}

#[tokio::test]
async fn streamed_frames_are_drained_before_after_response_and_replay_in_order() {
    let f = Fixture::new("review").await;
    let p = f.prepared().await;
    let intent = f.intent(&p).await;
    let OperationPhase::AssistantEffectPending {
        response_entry_id, ..
    } = &intent.phase
    else {
        panic!("intent")
    };
    let lane = Arc::clone(&f.lane);
    let run_id = f.drive.operation_id().to_owned();
    let response_id = response_entry_id.clone();
    f.lane.hooks().on(HookName::AfterResponse,Arc::new(move |invocation,_| {
        let HookEvent::AfterResponse(event) = invocation.event else {panic!("after response")};
        let lane = Arc::clone(&lane); let run_id = run_id.clone(); let response_id = response_id.clone();
        Box::pin(async move {
            let frames = lane.command(move |_,reader| { let run_id=run_id.clone(); let response_id=response_id.clone(); Box::pin(async move {
                Ok(crate::agent_core::harness::runtime::lane::LaneCommand::Return {
                    result:crate::agent_core::harness::runtime::progress::read_assistant_frames(reader,&run_id,&response_id,background_context()).await?
                })
            })}, background_context()).await?;
            let frames: Vec<crate::ai::frame::AssistantMessageFrame> = frames.into_iter().map(|frame| serde_json::from_value(frame).unwrap()).collect();
            let partial = crate::ai::frame::reduce_assistant_message_frames(&frames)?.expect("start persisted");
            assert_eq!(partial.content,event.message.content);
            Ok(HookResult::AfterResponse(None))
        })
    }),None).unwrap();
    f.queue("a streamed response containing enough chunks to exercise persistence order");
    let result = perform_generation(&f.lane, &f.drive, &intent, &p)
        .await
        .unwrap();
    assert_eq!(result.stop_reason, crate::ai::types::StopReason::Stop);
    assert!(f.frames(response_entry_id).await.len() >= 4);
}

#[tokio::test]
async fn provider_receives_the_gate_signal_and_abort_settlement_waits_for_cancellation_commit() {
    let f = Fixture::new("main").await;
    let p = f.prepared().await;
    let intent = f.intent(&p).await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let hook_entered = Arc::clone(&entered);
    f.faux
        .set_responses(vec![FauxResponseStep::Factory(Arc::new(move |args| {
            let signal = args
                .options
                .expect("options")
                .stream
                .signal
                .expect("gate signal");
            let entered = Arc::clone(&hook_entered);
            Box::pin(async move {
                assert!(!signal.is_cancelled());
                entered.notify_one();
                signal.cancelled().await;
                Ok(faux_assistant_message(
                    "raw after cancel",
                    Default::default(),
                ))
            })
        }))]);
    let cancellation = CancellationToken::new();
    let running = perform_generation(&f.lane, &f.drive, &intent, &p);
    tokio::pin!(running);
    tokio::select! { result = &mut running => panic!("provider finished before signal: {result:?}"), _ = entered.notified() => {} }
    f.drive.begin_abort(cancellation.clone());
    f.drive.signal_abort();
    // The gate is aborting, but the durable cancellation promise is unresolved.
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(30), &mut running)
            .await
            .is_err()
    );
    cancellation.cancel();
    let message = tokio::time::timeout(std::time::Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(message.stop_reason, crate::ai::types::StopReason::Aborted);
    assert!(f.events.lock().unwrap().iter().any(|event| matches!(event,HarnessEvent::MessageEnd {message:AgentMessage::Assistant(message),..} if message.stop_reason==crate::ai::types::StopReason::Aborted)));
}

#[tokio::test]
async fn response_publication_carries_latest_assistant_through_retry_and_tools() {
    use crate::ai::models::faux::{faux_tool_call, FauxMessageOptions, FauxToolCallOptions};
    for tools in [false, true] {
        let f = Fixture::new("main").await;
        let response = if tools {
            faux_assistant_message(
                faux_tool_call("lookup", json!({}), FauxToolCallOptions::default()),
                FauxMessageOptions {
                    stop_reason: Some(crate::ai::types::StopReason::ToolUse),
                    ..Default::default()
                },
            )
        } else {
            faux_assistant_message(
                "",
                FauxMessageOptions {
                    stop_reason: Some(crate::ai::types::StopReason::Error),
                    error_message: Some("429 rate limit exceeded".into()),
                    ..Default::default()
                },
            )
        };
        f.faux.append_responses(vec![response.into()]);
        assert!(matches!(
            run_generation(&f.lane, &f.drive, &f.state()).await.unwrap(),
            ProcedureResult::Continue
        ));
        let state = f.state();
        let latest = state
            .scope
            .latest_assistant_entry_id
            .as_deref()
            .expect("published response must update scope");
        assert_eq!(f.lane.state().tip_id.as_deref(), Some(latest));
        if tools {
            assert!(
                matches!(&state.phase,OperationPhase::Tools {batch} if batch.assistant_entry_id==latest)
            );
        } else {
            assert!(matches!(
                state.phase,
                OperationPhase::AssistantRetryWait { .. }
            ));
        }
    }
}

#[tokio::test]
async fn generation_hooks_reach_the_real_http_payload_and_response_metadata() {
    use crate::agent_core::harness::hooks::PayloadHookResult;
    use crate::ai::models::provider::{create_provider, ApiImpls, CreateProviderOptions};
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/chat/completions"))
        .and(wiremock::matchers::body_json(json!({"patchedByHarness":true})))
        .respond_with(wiremock::ResponseTemplate::new(200)
            .insert_header("content-type","text/event-stream")
            .insert_header("x-harness-response","local")
            .set_body_string("data: {\"id\":\"local\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"HTTP response\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"))
        .expect(1).mount(&server).await;
    let faux = faux_provider(FauxProviderOptions::default());
    let mut model = faux.provider.get_models().unwrap().remove(0);
    model.id = "local-http".into();
    model.provider = "local-http".into();
    model.api = "openai-completions".into();
    model.base_url = format!("{}/v1", server.uri());
    let provider = create_provider(CreateProviderOptions {
        filter_all_models: None,
        images: crate::ai::models::provider::ImagesImpls::new(),
        classifiers: crate::ai::models::provider::ClassifiersImpls::new(),
        id: model.provider.clone(),
        name: None,
        base_url: None,
        headers: None,
        auth: faux.provider.auth().clone(),
        models: vec![crate::ai::types::AnyModel::Chat(model)],
        fetch_models: None,
        filter_models: None,
        api: ApiImpls::Single(Arc::new(
            crate::ai::api::openai_completions::stream::OpenAiCompletions,
        )),
    });
    let f = Fixture::with_provider("review", Some(provider)).await;
    let mut ready = f.state();
    configured(&mut ready).model = LaneModel {
        provider: "local-http".into(),
        model_id: "local-http".into(),
    };
    f.set_state(ready).await;
    let trace = Arc::new(Mutex::new(Vec::new()));
    let payload_trace = Arc::clone(&trace);
    f.lane
        .hooks()
        .on(
            HookName::BeforePayload,
            Arc::new(move |invocation, context| {
                assert_eq!(invocation.lane, "review");
                assert!(context.abort_signal().is_some());
                let HookEvent::BeforePayload(event) = invocation.event else {
                    panic!("payload")
                };
                assert_eq!(event.model.id, "local-http");
                assert_eq!(event.payload["model"], "local-http");
                payload_trace.lock().unwrap().push("payload");
                Box::pin(async {
                    Ok(HookResult::BeforePayload(Some(PayloadHookResult {
                        payload: json!({"patchedByHarness":true}),
                    })))
                })
            }),
            None,
        )
        .unwrap();
    let response_trace = Arc::clone(&trace);
    f.lane
        .hooks()
        .on(
            HookName::AfterResponse,
            Arc::new(move |invocation, _| {
                let HookEvent::AfterResponse(event) = invocation.event else {
                    panic!("response")
                };
                assert_eq!(event.status, Some(200));
                assert_eq!(
                    event
                        .headers
                        .unwrap()
                        .get("x-harness-response")
                        .map(String::as_str),
                    Some("local")
                );
                assert_eq!(
                    event.message.stop_reason,
                    crate::ai::types::StopReason::Stop
                );
                response_trace.lock().unwrap().push("response");
                Box::pin(async { Ok(HookResult::AfterResponse(None)) })
            }),
            None,
        )
        .unwrap();
    let mut p = f.prepared().await;
    p.stream_options.headers = Some(std::collections::BTreeMap::from([(
        "Authorization".into(),
        "Bearer local-test-only".into(),
    )]));
    let intent = f.intent(&p).await;
    let message = perform_generation(&f.lane, &f.drive, &intent, &p)
        .await
        .unwrap();
    assert_eq!(message.stop_reason, crate::ai::types::StopReason::Stop);
    assert_eq!(*trace.lock().unwrap(), vec!["payload", "response"]);
}

#[tokio::test]
async fn preparation_matches_actual_upstream_typescript_oracle() {
    let oracle: serde_json::Value = serde_json::from_str(include_str!("oracle.json")).unwrap();
    for case in oracle["preparation"].as_array().unwrap() {
        let input = &case["input"];
        let f = Fixture::new("review").await;
        let mut ready = f.state();
        if let Some(id) = input["modelId"].as_str() {
            configured(&mut ready).model.model_id = id.into();
        }
        configured(&mut ready).active_tool_names =
            serde_json::from_value(input.get("active").cloned().unwrap_or(json!([]))).unwrap();
        f.config.lock().unwrap().tools =
            serde_json::from_value(input.get("tools").cloned().unwrap_or(json!([]))).unwrap();
        f.config.lock().unwrap().system_prompt = input["prompt"].as_str().map(str::to_owned);
        if let OperationPhase::AssistantReady {
            generation_context, ..
        } = &mut ready.phase
        {
            generation_context.stream_options =
                serde_json::from_value(input.get("base").cloned().unwrap_or(json!({}))).unwrap();
        }
        if input["cancel"] == true {
            ready.scope.control = Control::CancelRequested { requested_at: 3 };
        }
        if let Some(patch) = input.get("patch") {
            let patch: AgentHarnessStreamOptionsPatch =
                serde_json::from_value(patch.clone()).unwrap();
            f.lane
                .hooks()
                .on(
                    HookName::BeforeRequest,
                    Arc::new(move |_, _| {
                        let patch = patch.clone();
                        Box::pin(async move {
                            Ok(HookResult::BeforeRequest(Some(BeforeRequestHookResult {
                                stream_options: patch,
                            })))
                        })
                    }),
                    None,
                )
                .unwrap();
        }
        f.set_state(ready.clone()).await;
        let result = prepare_generation(&f.lane, &f.drive, &ready).await.unwrap();
        let actual = match result {
            GenerationPreparation::Ready(prepared) => {
                json!({"kind":"ready","tools":prepared.tools,"systemPrompt":prepared.system_prompt,"streamOptions":prepared.stream_options})
            }
            GenerationPreparation::ConfigurationFailure { error } => {
                json!({"kind":"configuration_failure","error":error})
            }
            GenerationPreparation::CancelRequested => json!({"kind":"cancel_requested"}),
        };
        assert_eq!(actual, case["output"], "case {}", input["name"]);
    }
}

#[tokio::test]
async fn intent_matches_actual_upstream_typescript_oracle() {
    let oracle: serde_json::Value = serde_json::from_str(include_str!("oracle.json")).unwrap();
    for case in oracle["intent"].as_array().unwrap() {
        let f = Fixture::new("review").await;
        let mut ready = f.state();
        let attempt = case["attempt"].as_u64().unwrap() as u32;
        let OperationPhase::AssistantReady {
            next_attempt,
            generation_context,
        } = &mut ready.phase
        else {
            panic!("ready")
        };
        *next_attempt = attempt;
        let turn_id = generation_context.step_id.clone();
        f.set_state(ready.clone()).await;
        let mut prepared = f.prepared().await;
        prepared.model.max_tokens = 1234;
        prepared.model.context_window = 45678;
        let mut current = ready.clone();
        current.scope.latest_assistant_entry_id = Some("current-latest".into());
        f.set_state(current.clone()).await;
        let ContinueOperationResult::Result { value: pending } =
            publish_generation_intent(&f.lane, &f.drive, &ready, &prepared)
                .await
                .unwrap()
        else {
            panic!("intent")
        };
        let OperationPhase::AssistantEffectPending {
            attempt,
            response_entry_id,
            usage_id,
            intended_output_limit,
            context_window,
            ..
        } = &pending.phase
        else {
            panic!("pending")
        };
        let events:Vec<_>=f.events.lock().unwrap().iter().map(|event|match event {
            HarnessEvent::TurnStart {lane,run_id,turn_id:actual_turn,..}=>json!({"type":"turn_start","lane":lane,"runIdMatches":run_id==f.drive.operation_id(),"turnIdMatches":actual_turn==&turn_id}),
            event=>panic!("unexpected {event:?}"),
        }).collect();
        let actual = json!({"at":"assistant.effect_pending","attempt":attempt,"intendedOutputLimit":intended_output_limit,"contextWindow":context_window,"scopePreserved":pending.scope==current.scope,"distinctIds":response_entry_id!=usage_id,"sameTimestamp":super::super::response::uuid_v7_timestamp(response_entry_id).unwrap()==super::super::response::uuid_v7_timestamp(usage_id).unwrap(),"events":events});
        assert_eq!(actual, case["output"]);
    }
}

#[tokio::test]
async fn retry_matches_actual_upstream_typescript_oracle() {
    let oracle: serde_json::Value = serde_json::from_str(include_str!("oracle.json")).unwrap();
    for case in oracle["retry"].as_array().unwrap() {
        let f = Fixture::new("review").await;
        let deadline = if case["future"] == true {
            crate::ai::now_ms() + 60_000
        } else {
            0
        };
        let retry = f.retry(deadline).await;
        let OperationPhase::AssistantRetryWait {
            generation_context, ..
        } = &retry.phase
        else {
            panic!("retry")
        };
        let result = run_retry_wait(&f.lane, &f.drive, &retry).await.unwrap();
        let state = f.state();
        let actual = match result {
            ProcedureResult::Waiting {
                outcome:
                    DriveOutcome::Waiting {
                        operation_id,
                        reason: WaitingReason::Retry { not_before },
                    },
            } => {
                json!({"kind":"waiting","reason":"retry","deadlinePreserved":not_before==deadline,"runIdMatches":operation_id==f.drive.operation_id(),"unchanged":state==retry,"events":f.events.lock().unwrap().len()})
            }
            ProcedureResult::Continue => {
                let OperationPhase::AssistantReady { next_attempt, .. } = &state.phase else {
                    panic!("ready")
                };
                let events:Vec<_>=f.events.lock().unwrap().iter().map(|event|match event {
                    HarnessEvent::RetryStart {lane,run_id,step,attempt}=>json!({"type":"retry_start","lane":lane,"runIdMatches":run_id==f.drive.operation_id(),"stepMatches":step==&generation_context.step_id,"attempt":attempt}),
                    event=>panic!("unexpected {event:?}"),
                }).collect();
                json!({"kind":"continue","at":"assistant.ready","nextAttempt":next_attempt,"scopePreserved":state.scope==retry.scope,"events":events})
            }
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(actual, case["output"]);
    }
}

#[tokio::test]
async fn orphaned_generation_replays_committed_prefix_without_a_second_provider_call() {
    use crate::agent_core::harness::runtime::drive::recovery::recover_assistant_generation;
    let f = Fixture::new("review").await;
    let prepared = f.prepared().await;
    let intent = f.intent(&prepared).await;
    f.queue("committed prefix");
    let original = perform_generation(&f.lane, &f.drive, &intent, &prepared)
        .await
        .unwrap();
    assert_eq!(f.faux.state().lock().unwrap().call_count, 1);
    f.events.lock().unwrap().clear();
    // The intent is still durable, as if the process exited before publication.
    recover_assistant_generation(&f.lane, &f.drive, &intent)
        .await
        .unwrap();
    assert_eq!(f.faux.state().lock().unwrap().call_count, 1);
    let events = f.events.lock().unwrap();
    let messages = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                HarnessEvent::MessageStart { .. } | HarnessEvent::MessageEnd { .. }
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(messages.len(), 2);
    for event in messages {
        let wire = serde_json::to_value(event).unwrap();
        assert_eq!(wire["lane"], "review");
        assert_eq!(wire["runId"], f.drive.operation_id());
        assert_eq!(wire["recovery"], true);
        let message: AssistantMessage = serde_json::from_value(wire["message"].clone()).unwrap();
        assert_eq!(message.content, original.content);
        assert_eq!(message.stop_reason, crate::ai::types::StopReason::Error);
        assert_eq!(message.usage, Default::default());
        assert!(message
            .error_message
            .as_deref()
            .unwrap()
            .contains("Assistant request was interrupted"));
    }
}

mod anthropic_callbacks;
