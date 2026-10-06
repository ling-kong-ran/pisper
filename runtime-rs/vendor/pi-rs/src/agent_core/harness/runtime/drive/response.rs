//! Port of the structural-independent subset of
//! `packages/agent/src/harness/runtime/drive/response.ts`: the response
//! classification helpers (`response.ts:140-179`) and
//! `publishConfigurationFailure` (`response.ts:105-138`).
//!
//! Also ports `openAssistantResponse` / `AssistantResponseLifecycle`
//! (`response.ts:36-97`) over the staged frame channel and the gated
//! `after_response` hook chain.
//!
//! Response publication lives in sibling `publish`; recovery and structural
//! preparation are separate modules. See migration status for integration gaps.

use std::sync::Arc;

use crate::agent_core::harness::context::Context;
use crate::agent_core::harness::runtime::drive::terminal::{
    operation_cleanup_writes, operation_result_record,
};
use crate::agent_core::harness::runtime::drive_pass::{Drive, ProcedureResult};
use crate::agent_core::harness::runtime::durable::OperationState;
use crate::agent_core::harness::runtime::events::{HarnessEvent, RunEndStatus};
use crate::agent_core::harness::runtime::lane::{ContinueOperationResult, Lane, OperationCommand};
use crate::agent_core::harness::runtime::progress;
use crate::agent_core::harness::session::types::TerminalStatus;
use crate::agent_core::harness::session::types::{LaneModel, OperationError};
use crate::agent_core::types::AgentMessage;
use crate::ai::types::message::AssistantMessage;
use crate::ai::types::StopReason;

/// Upstream `"assistant" | "deferred"` source discriminator
/// (`response.ts:146, 159`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseSource {
    Assistant,
    Deferred,
}

impl ResponseSource {
    /// The capitalized label used in upstream fallback messages.
    pub(crate) fn label(self) -> &'static str {
        match self {
            ResponseSource::Assistant => "Assistant",
            ResponseSource::Deferred => "Deferred",
        }
    }
}

/// Upstream `uuidV7Timestamp` (`response.ts:140-144`): the unix-millisecond
/// timestamp encoded in a reserved UUIDv7 id's first 12 hex digits.
pub fn uuid_v7_timestamp(id: &str) -> anyhow::Result<i64> {
    let bytes: Result<i64, _> = i64::from_str_radix(
        &(id.chars()
            .take(13)
            .filter(|ch| *ch != '-')
            .collect::<String>()),
        16,
    );
    match bytes {
        Ok(timestamp) if (0..=9_007_199_254_740_991).contains(&timestamp) => Ok(timestamp),
        _ => anyhow::bail!("Invalid reserved UUIDv7 {id}"),
    }
}

/// Upstream `providerError` (`response.ts:146-153`).
pub fn provider_error(source: ResponseSource, message: &AssistantMessage) -> OperationError {
    OperationError {
        code: "assistant_error".to_string(),
        message: message.error_message.clone().unwrap_or_else(|| {
            format!(
                "{} request ended with {:?}",
                source.label(),
                message.stop_reason
            )
        }),
        details: None,
    }
}

/// Upstream `normalizeError` (`response.ts:155-157`).
pub fn normalize_error(message: &AssistantMessage, error_message: &str) -> AssistantMessage {
    let mut normalized = message.clone();
    normalized.stop_reason = StopReason::Error;
    normalized.error_message = Some(error_message.to_string());
    normalized
}

/// Upstream `normalizeAborted` (`response.ts:159-166`).
pub fn normalize_aborted(source: ResponseSource, message: &AssistantMessage) -> AssistantMessage {
    let mut normalized = message.clone();
    normalized.stop_reason = StopReason::Aborted;
    normalized.error_message = Some(
        message
            .error_message
            .clone()
            .unwrap_or_else(|| format!("{} request was cancelled", source.label())),
    );
    normalized
}

/// Upstream `deferredHandleIsValid` (`response.ts:168-179`): the response's
/// deferred handle must agree with the generating configuration. `model` is
/// the generation context's configured model identity.
pub fn deferred_handle_is_valid(message: &AssistantMessage, model: &LaneModel) -> bool {
    if message.stop_reason != StopReason::Deferred {
        return false;
    }
    match &message.deferred {
        Some(handle) => {
            !handle.id.is_empty()
                && handle.provider == model.provider
                && handle.model_id == model.model_id
                && handle.api == message.api
        }
        None => false,
    }
}

/// Upstream `publishConfigurationFailure` (`response.ts:105-138`): publish a
/// non-retryable request-configuration failure before reserving response ids.
pub async fn publish_configuration_failure(
    lane: &std::sync::Arc<Lane>,
    drive: &Drive,
    _capability: &OperationState,
    error: OperationError,
) -> anyhow::Result<ProcedureResult> {
    let context = drive.context().clone();
    let call_context = context.clone();
    let operation_id = drive.operation_id().to_string();
    let lane_name = lane.name().to_string();
    let run_id = operation_id.clone();
    let result = lane
        .continue_operation(
            move |state, current, meta, reader| {
                let error = error.clone();
                let context = context.clone();
                let operation_id = operation_id.clone();
                let lane_name = lane_name.clone();
                let run_id = run_id.clone();
                let source_tip_id = meta.source_tip_id.clone();
                Box::pin(async move {
                    let Some(tip_id) = state.tip_id.clone() else {
                        anyhow::bail!("Failed run has no Branch tip");
                    };
                    let record = operation_result_record(
                        meta,
                        TerminalStatus::Failed,
                        Some(tip_id.clone()),
                        Some(error.clone()),
                    )?;
                    let cleanup =
                        operation_cleanup_writes(reader, &operation_id, current, context.clone())
                            .await?;
                    let ended_at = record.ended_at;
                    Ok(OperationCommand::Finish {
                        writes: cleanup,
                        record: record.clone(),
                        lane: None,
                        materialize: Box::new(move |_commit| ProcedureResult::Settled {
                            outcome: record,
                        }),
                        events: Some(Box::new(
                            move |_commit: &crate::agent_core::harness::session::CommitResult| {
                                Ok(vec![HarnessEvent::RunEnd {
                                    lane: lane_name,
                                    run_id,
                                    status: RunEndStatus::Failed,
                                    error: Some(error),
                                    from_tip_id: source_tip_id,
                                    tip_id: Some(tip_id.clone()),
                                    ended_at,
                                }])
                            },
                        )),
                    })
                })
            },
            call_context,
        )
        .await?;
    Ok(match result {
        ContinueOperationResult::CancelRequested => ProcedureResult::Continue,
        ContinueOperationResult::Result { value } => value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settled_message(json: &str) -> AssistantMessage {
        serde_json::from_str(json).unwrap()
    }

    fn base_message() -> AssistantMessage {
        settled_message(
            r#"{"role":"assistant","content":[],"api":"faux","provider":"faux","model":"faux-1","stopReason":"stop","errorMessage":"boom","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"timestamp":9}"#,
        )
    }

    #[test]
    fn uuid_v7_timestamp_extracts_unix_millis() {
        // The first 12 hex digits are the unix-millisecond timestamp:
        // 0x018b3d1e8000 = 1697537490944 (2023-11-17T07:51:30.944Z).
        let id = "018b3d1e-8000-7abc-9def-0123456789ab";
        let timestamp = uuid_v7_timestamp(id).unwrap();
        assert_eq!(timestamp, 1_697_537_490_944);
        let error = uuid_v7_timestamp("not-a-uuid").unwrap_err();
        assert!(error.to_string().contains("Invalid reserved UUIDv7"));
    }

    #[test]
    fn provider_error_prefers_the_error_message() {
        let message = base_message();
        let error = provider_error(ResponseSource::Assistant, &message);
        assert_eq!(error.code, "assistant_error");
        assert_eq!(error.message, "boom");
        // Without an errorMessage the stop-reason fallback carries the source
        // label.
        let mut quiet = message.clone();
        quiet.error_message = None;
        quiet.stop_reason = StopReason::Length;
        let error = provider_error(ResponseSource::Deferred, &quiet);
        assert_eq!(
            error.message,
            "Deferred request ended with Length".to_string()
        );
    }

    #[test]
    fn normalize_error_and_aborted_follow_upstream_defaults() {
        let mut message = base_message();
        message.error_message = None;
        let errored = normalize_error(&message, "exceeded");
        assert_eq!(errored.stop_reason, StopReason::Error);
        assert_eq!(errored.error_message.as_deref(), Some("exceeded"));
        let aborted = normalize_aborted(ResponseSource::Assistant, &message);
        assert_eq!(aborted.stop_reason, StopReason::Aborted);
        assert_eq!(
            aborted.error_message.as_deref(),
            Some("Assistant request was cancelled")
        );
    }

    #[test]
    fn deferred_handle_validity_checks_identity_fields() {
        let model = LaneModel {
            provider: "faux".to_string(),
            model_id: "faux-1".to_string(),
        };
        let mut message = base_message();
        message.stop_reason = StopReason::Deferred;
        // No handle at all.
        assert!(!deferred_handle_is_valid(&message, &model));
        // Handle with the right identity.
        message.deferred = Some(crate::ai::types::options::DeferredHandle {
            provider: "faux".to_string(),
            model_id: "faux-1".to_string(),
            api: "faux".to_string(),
            id: "tok_1".to_string(),
            expires_at: None,
            data: None,
            poll_after_ms: None,
        });
        assert!(deferred_handle_is_valid(&message, &model));
        // Empty id is invalid.
        message.deferred.as_mut().unwrap().id = String::new();
        assert!(!deferred_handle_is_valid(&message, &model));
        // Mismatched provider is invalid.
        message.deferred.as_mut().unwrap().id = "tok_1".to_string();
        message.deferred.as_mut().unwrap().provider = "other".to_string();
        assert!(!deferred_handle_is_valid(&message, &model));
        // Non-deferred stop reason is invalid regardless.
        message.deferred.as_mut().unwrap().provider = "faux".to_string();
        message.stop_reason = StopReason::Stop;
        assert!(!deferred_handle_is_valid(&message, &model));
    }
}

// --- assistant response lifecycle (response.ts:36-97) ---

/// Adapter exposing the lifecycle observer trio as the
/// [`AssistantStreamObserver`] trait object `consume_assistant_stream` drives.
pub struct LifecycleObserver(pub Arc<AssistantResponseLifecycle>);

impl crate::agent_core::harness::execution::assistant::AssistantStreamObserver
    for LifecycleObserver
{
    fn try_start<'a>(
        &'a self,
        message: AssistantMessage,
        event: crate::ai::types::events::AssistantMessageEvent,
        context: Context,
    ) -> futures::future::BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            self.0
                .start(&AgentMessage::Assistant(message), &event, context)
                .await
        })
    }
    fn try_update<'a>(
        &'a self,
        message: AssistantMessage,
        event: crate::ai::types::events::AssistantMessageEvent,
        context: Context,
    ) -> futures::future::BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            self.0
                .update(&AgentMessage::Assistant(message), &event, context)
                .await
        })
    }
    fn try_end<'a>(
        &'a self,
        message: AssistantMessage,
        context: Context,
    ) -> futures::future::BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move { self.0.end(&AgentMessage::Assistant(message), context).await })
    }

    fn start<'a>(
        &'a self,
        message: AssistantMessage,
        event: crate::ai::types::events::AssistantMessageEvent,
        context: Context,
    ) -> futures::future::BoxFuture<'a, ()> {
        let agent_message = AgentMessage::Assistant(message);
        Box::pin(async move {
            // Upstream observer methods are infallible event forwards; an
            // emit failure surfaces at the next observer call.
            let _ = self.0.start(&agent_message, &event, context).await;
        })
    }

    fn update<'a>(
        &'a self,
        message: AssistantMessage,
        event: crate::ai::types::events::AssistantMessageEvent,
        context: Context,
    ) -> futures::future::BoxFuture<'a, ()> {
        let agent_message = AgentMessage::Assistant(message);
        Box::pin(async move {
            let _ = self.0.update(&agent_message, &event, context).await;
        })
    }

    fn end<'a>(
        &'a self,
        message: AssistantMessage,
        context: Context,
    ) -> futures::future::BoxFuture<'a, ()> {
        let agent_message = AgentMessage::Assistant(message);
        Box::pin(async move {
            let _ = self.0.end(&agent_message, context).await;
        })
    }
}

pub use crate::agent_core::harness::execution::assistant::AssistantResponseMetadata;

/// Upstream `AssistantResponseLifecycle` (`response.ts:36-44`): the
/// observer trio plus `after_response` and `close`, bound to one response
/// entry's staged frame channel.
pub struct AssistantResponseLifecycle {
    lane: std::sync::Arc<Lane>,
    run_id: String,
    lane_name: String,
    response_entry_id: String,
    recovery: bool,
    encoder: std::sync::Mutex<crate::ai::frame::AssistantMessageFrameEncoder>,
    progress: progress::ProgressChannel<crate::ai::frame::AssistantMessageFrame>,
}

impl AssistantResponseLifecycle {
    /// Upstream `openAssistantResponse` (`response.ts:46-62`).
    pub fn open(
        lane: std::sync::Arc<Lane>,
        drive: &Drive,
        response_entry_id: &str,
        recovery: bool,
    ) -> Self {
        let progress = progress::open_frame_progress(&lane, drive, response_entry_id);
        AssistantResponseLifecycle {
            lane_name: lane.name().to_string(),
            lane,
            run_id: drive.operation_id().to_string(),
            response_entry_id: response_entry_id.to_string(),
            recovery,
            encoder: std::sync::Mutex::new(crate::ai::frame::AssistantMessageFrameEncoder::new()),
            progress,
        }
    }

    fn event_lane(&self) -> String {
        self.lane_name.clone()
    }

    /// Upstream `observer.start` (`response.ts:65-69`): encode the frame,
    /// stage it, and emit `message_start`.
    pub async fn start(
        &self,
        message: &AgentMessage,
        event: &crate::ai::types::events::AssistantMessageEvent,
        context: Context,
    ) -> anyhow::Result<()> {
        let frame = self.encode_frame(event)?;
        self.emit_start(message, frame.as_ref(), context).await
    }

    /// Upstream `observer.update` (`response.ts:70-77`).
    pub async fn update(
        &self,
        message: &AgentMessage,
        event: &crate::ai::types::events::AssistantMessageEvent,
        context: Context,
    ) -> anyhow::Result<()> {
        let frame = self.encode_frame(event)?;
        self.emit_update(message, event, frame.as_ref(), context)
            .await
    }

    /// Upstream `observer.end` (`response.ts:78-83`).
    pub async fn end(&self, message: &AgentMessage, context: Context) -> anyhow::Result<()> {
        self.lane
            .emit_batch(
                vec![HarnessEvent::MessageEnd {
                    recovery: self.recovery,
                    lane: self.event_lane(),
                    run_id: Some(self.run_id.clone()),
                    message: message.clone(),
                    entry_id: self.response_entry_id.clone(),
                }],
                context,
            )
            .await
    }

    /// Upstream `afterResponse` (`response.ts:85-94`): close the channel,
    /// run the gated `after_response` hook chain, and take a replacement
    /// message when a handler supplies one.
    pub async fn after_response(
        &self,
        metadata: &AssistantResponseMetadata,
        message: &AssistantMessage,
        gate: Arc<dyn crate::agent_core::harness::hooks::Gate>,
        context: Context,
    ) -> anyhow::Result<AssistantMessage> {
        self.close().await?;
        let hook = self
            .lane
            .hooks()
            .run_with_gate(
                crate::agent_core::harness::hooks::HookInvocation {
                    lane: self.event_lane(),
                    run_id: self.run_id.clone(),
                    event: crate::agent_core::harness::hooks::HookEvent::AfterResponse(
                        crate::agent_core::harness::hooks::AfterResponseEvent {
                            status: metadata.status,
                            headers: metadata.headers.clone(),
                            message: message.clone(),
                        },
                    ),
                },
                gate,
                context,
            )
            .await?;
        Ok(match &hook {
            crate::agent_core::harness::hooks::HookResult::AfterResponse(Some(result)) => {
                result.message.clone()
            }
            _ => message.clone(),
        })
    }

    /// Upstream lifecycle `close` (`response.ts:59-62, 95`): seal the frame
    /// channel and drain the newest write.
    pub async fn close(&self) -> anyhow::Result<()> {
        self.progress.seal();
        self.progress.drain().await
    }

    fn encode_frame(
        &self,
        event: &crate::ai::types::events::AssistantMessageEvent,
    ) -> anyhow::Result<Option<crate::ai::frame::AssistantMessageFrame>> {
        let mut encoder = self
            .encoder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        encoder.encode(event)
    }

    async fn emit_start(
        &self,
        message: &AgentMessage,
        frame: Option<&crate::ai::frame::AssistantMessageFrame>,
        context: Context,
    ) -> anyhow::Result<()> {
        if let Some(frame) = frame {
            self.progress.write(frame.clone());
        }
        self.lane
            .emit_batch(
                vec![HarnessEvent::MessageStart {
                    recovery: self.recovery,
                    lane: self.event_lane(),
                    run_id: Some(self.run_id.clone()),
                    message: message.clone(),
                }],
                context,
            )
            .await
    }

    async fn emit_update(
        &self,
        message: &AgentMessage,
        event: &crate::ai::types::events::AssistantMessageEvent,
        frame: Option<&crate::ai::frame::AssistantMessageFrame>,
        context: Context,
    ) -> anyhow::Result<()> {
        if let Some(frame) = frame {
            self.progress.write(frame.clone());
        }
        self.lane
            .emit_batch(
                vec![HarnessEvent::MessageUpdate {
                    recovery: self.recovery,
                    lane: self.event_lane(),
                    run_id: self.run_id.clone(),
                    message: message.clone(),
                    event: event.clone(),
                    frame: frame.cloned(),
                }],
                context,
            )
            .await
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use crate::agent_core::harness::execution::create_gate;
    use crate::agent_core::harness::runtime::drive_pass::DriveOptions;
    use crate::agent_core::harness::runtime::lane::{EmitBatch, RuntimeConfig};
    use crate::agent_core::harness::runtime::restore::restore_lane;
    use crate::agent_core::harness::session;
    use crate::agent_core::harness::session::values::{branch_tip, set_value};
    use crate::agent_core::harness::session::Write;
    use crate::agent_core::harness::session::{LaneConfiguration, Session as _};
    use crate::agent_core::harness::session::{
        LaneModel, MemoryStorage, MemoryStorageOptions, SessionMetadata, StorageBackedSession,
    };
    use crate::agent_core::harness::{background_context, DEFAULT_COMPACTION_SETTINGS};
    use crate::agent_core::types::{QueueMode, ThinkingLevel, ToolExecutionMode};
    use crate::ai::frame::AssistantMessageFrame;
    use crate::ai::models::faux::{faux_provider, FauxProviderOptions};
    use crate::ai::models::{create_models, CreateModelsOptions};
    use std::sync::Mutex;

    fn assistant_fixture() -> AssistantMessage {
        serde_json::from_str(
            r#"{"role":"assistant","content":[],"api":"faux","provider":"faux","model":"faux-1","stopReason":"stop","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"timestamp":9}"#,
        )
        .unwrap()
    }

    async fn create_capturing_lane(events: Arc<Mutex<Vec<HarnessEvent>>>) -> std::sync::Arc<Lane> {
        let events_for_emit = Arc::clone(&events);
        let storage = MemoryStorage::new(MemoryStorageOptions::default());
        let sess = Arc::new(StorageBackedSession::new(
            SessionMetadata {
                id: "response-lifecycle-test".into(),
                created_at: 1,
                storage_version: 1,
                ..Default::default()
            },
            Arc::new(storage),
        ));
        let writes: Vec<Write> = vec![
            set_value(&branch_tip("main"), serde_json::Value::Null),
            set_value(
                &session::lane_config("main"),
                session::lane_configuration_value(&LaneConfiguration {
                    model: LaneModel {
                        provider: "faux".to_string(),
                        model_id: "faux-1".to_string(),
                    },
                    thinking_level: ThinkingLevel::Off,
                    active_tool_names: Vec::new(),
                }),
            ),
            set_value(
                &session::lane_state("main"),
                serde_json::json!({"currentOperationId":null,"lastOperationId":null,"inbox":[]}),
            ),
        ];
        sess.mutate(
            move |reader, context| {
                Box::pin(async move { reader.commit(writes, context).await.map(|_| ()) })
            },
            background_context(),
        )
        .await
        .unwrap();
        let faux = faux_provider(FauxProviderOptions::default());
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(Arc::clone(&faux.provider));
        let state = restore_lane(sess.as_ref(), "main", background_context())
            .await
            .expect("lane restores");
        let emit: EmitBatch = Arc::new(move |batch, _context| {
            let events = Arc::clone(&events_for_emit);
            Box::pin(async move {
                events.lock().unwrap().extend(batch);
                Ok(())
            })
        });
        Lane::new(
            "main",
            sess,
            models,
            crate::agent_core::harness::hooks::HookRegistry::new(Arc::new(
                |_error: anyhow::Error,
                 _hook: crate::agent_core::harness::hooks::HookName,
                 _message: String,
                 _context| { Box::pin(async {}) },
            )),
            state,
            Arc::new(|error: anyhow::Error| error),
            emit,
            Arc::new(move || RuntimeConfig {
                compaction: DEFAULT_COMPACTION_SETTINGS,
                retry_policy: crate::agent_core::harness::config::DEFAULT_RETRY_POLICY,
                system_prompt: None,
                tools: Vec::new(),
                native_tools: Default::default(),
                to_provider_messages: None,
                resources: Default::default(),
                stream_options: Default::default(),
                steering_mode: QueueMode::All,
                follow_up_mode: QueueMode::All,
                tool_execution: ToolExecutionMode::Parallel,
                entry_projectors: None,
            }),
        )
    }

    #[tokio::test]
    async fn lifecycle_emits_start_update_end_and_after_response_passthrough() {
        check_lifecycle_context(false).await;
    }

    #[tokio::test]
    async fn recovered_lifecycle_preserves_run_id_and_recovery_on_all_message_events() {
        check_lifecycle_context(true).await;
    }

    async fn check_lifecycle_context(recovery: bool) {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let lane = create_capturing_lane(Arc::clone(&captured)).await;
        let drive = Drive::new(
            &DriveOptions {
                operation_id: "op1".to_string(),
                wait_for_retry: None,
                poll_deferred: None,
            },
            background_context(),
        );
        let lifecycle = AssistantResponseLifecycle::open(lane.clone(), &drive, "r1", recovery);
        let message = AgentMessage::Assistant(assistant_fixture());
        let start_event: crate::ai::types::events::AssistantMessageEvent =
            serde_json::from_value(serde_json::json!({
                "type": "start",
                "message": serde_json::to_value(assistant_fixture()).unwrap()
            }))
            .unwrap();
        lifecycle
            .start(&message, &start_event, background_context())
            .await
            .unwrap();
        let text_start_event: crate::ai::types::events::AssistantMessageEvent =
            serde_json::from_value(serde_json::json!({"type": "text_start", "contentIndex": 0}))
                .unwrap();
        lifecycle
            .update(&message, &text_start_event, background_context())
            .await
            .unwrap();
        let delta_event: crate::ai::types::events::AssistantMessageEvent = serde_json::from_value(
            serde_json::json!({"type": "text_delta", "contentIndex": 0, "delta": "hi"}),
        )
        .unwrap();
        lifecycle
            .update(&message, &delta_event, background_context())
            .await
            .unwrap();
        lifecycle.end(&message, background_context()).await.unwrap();

        {
            let events = captured.lock().unwrap();
            assert!(matches!(
                events.first(),
                Some(HarnessEvent::MessageStart { .. })
            ));
            // The start frame stages only; MessageStart carries no frame payload
            // (upstream start frame rides on the staged channel, not the event).
            assert!(matches!(
                events.get(1),
                Some(HarnessEvent::MessageUpdate {
                    frame: Some(AssistantMessageFrame::TextStart { .. }),
                    ..
                })
            ));
            assert!(matches!(
                events.get(2),
                Some(HarnessEvent::MessageUpdate {
                    frame: Some(AssistantMessageFrame::TextDelta { .. }),
                    ..
                })
            ));
            assert_eq!(events.len(), 4, "start, two updates, end");
            for event in events.iter() {
                let wire = serde_json::to_value(event).unwrap();
                assert_eq!(wire["runId"], "op1");
                assert_eq!(
                    wire.get("recovery"),
                    recovery.then_some(&serde_json::Value::Bool(true))
                );
                assert_eq!(
                    serde_json::from_value::<HarnessEvent>(wire).unwrap(),
                    *event
                );
            }
            assert!(matches!(
                events.last(),
                Some(HarnessEvent::MessageEnd { .. })
            ));
        }

        // No after_response handlers: identity passthrough.
        let (gate, _control) = create_gate();
        let settled = lifecycle
            .after_response(
                &AssistantResponseMetadata::default(),
                &assistant_fixture(),
                Arc::new(gate),
                background_context(),
            )
            .await
            .unwrap();
        assert_eq!(settled.stop_reason, StopReason::Stop);
    }
}
