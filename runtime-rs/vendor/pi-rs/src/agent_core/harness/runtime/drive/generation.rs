//! Assistant generation driver, ported from `runtime/drive/generation.ts`.
//! Durable intent is published before any provider effect. Request hooks run
//! through the drive gate, and the frame channel is drained on every exit.
//! Callable systemPrompt/toolContext and telemetry context remain unported.
//! Provider callback support is capability-checked by Models (not discarded).

use std::sync::Arc;

use super::response::{AssistantResponseLifecycle, LifecycleObserver};
use crate::agent_core::harness::context::with_abort_signal;
use crate::agent_core::harness::execution::assistant::{
    self, HarnessAssistantStreamConfig, HarnessRequestContext, ToProviderMessagesFn,
};
use crate::agent_core::harness::hooks::{
    BeforePayloadEvent, HookEvent, HookInvocation, HookResult, TransformContextEvent,
};
use crate::agent_core::harness::runtime::drive::response::publish_configuration_failure;
use crate::agent_core::harness::runtime::drive::retry::wait_until;
use crate::agent_core::harness::runtime::drive_pass::{Drive, ProcedureResult, WaitingReason};
use crate::agent_core::harness::runtime::durable::OperationPhase;
use crate::agent_core::harness::runtime::durable::OperationState;
use crate::agent_core::harness::runtime::events::HarnessEvent;
use crate::agent_core::harness::runtime::lane::{ContinueOperationResult, Lane, OperationCommand};
use crate::agent_core::harness::runtime::transcript::read_bounded_context;
use crate::agent_core::harness::session::types::{CommitResult, OperationError};
use crate::agent_core::harness::session::Session as _;
use crate::agent_core::types::AgentMessage;
use crate::ai::types::message::AssistantMessage;
use crate::ai::types::model::Model;

pub struct PreparedGeneration {
    pub model: Model,
    pub tools: Vec<crate::ai::types::tool::Tool>,
    pub messages: Vec<AgentMessage>,
    pub system_prompt: String,
    pub to_provider_messages: Arc<ToProviderMessagesFn>,
    pub stream_options: crate::agent_core::harness::types::AgentHarnessStreamOptions,
}

#[allow(clippy::large_enum_variant)]
pub enum GenerationPreparation {
    Ready(PreparedGeneration),
    ConfigurationFailure { error: OperationError },
    CancelRequested,
}

pub use crate::agent_core::harness::runtime::drive_pass::ProcedureResult as PublishOutcome;

fn configuration_error(code: &str, message: String, details: serde_json::Value) -> OperationError {
    OperationError {
        code: code.to_string(),
        message,
        details: Some(details),
    }
}

fn resolve_system_prompt(lane: &Lane) -> String {
    lane.read_config().system_prompt.clone().unwrap_or_default()
}

pub async fn prepare_generation(
    lane: &Lane,
    drive: &Drive,
    generation: &OperationState,
) -> anyhow::Result<GenerationPreparation> {
    let (identity, next_attempt, gen_stream_options, active_tool_names) = match &generation.phase {
        OperationPhase::AssistantReady {
            generation_context,
            next_attempt,
        } => (
            generation_context.configuration.model.clone(),
            *next_attempt,
            generation_context.stream_options.clone(),
            generation_context.configuration.active_tool_names.clone(),
        ),
        _ => anyhow::bail!("Generation preparation requires an assistant-ready operation"),
    };
    let model = lane
        .models()
        .get_model(&identity.provider, &identity.model_id);
    let Some(model) = model else {
        return Ok(GenerationPreparation::ConfigurationFailure {
            error: configuration_error(
                "model_unavailable",
                "The configured model is unavailable in this process".to_string(),
                serde_json::json!({
                    "provider": identity.provider,
                    "modelId": identity.model_id,
                }),
            ),
        });
    };

    let config = lane.read_config();
    let tools_by_name: std::collections::HashMap<&str, &crate::ai::types::tool::Tool> = config
        .tools
        .iter()
        .map(|tool| (tool.name.as_str(), tool))
        .collect();
    let missing_tools: Vec<&String> = active_tool_names
        .iter()
        .filter(|name| !tools_by_name.contains_key(name.as_str()))
        .collect();
    if !missing_tools.is_empty() {
        let missing: Vec<String> = missing_tools.iter().map(|name| (*name).clone()).collect();
        return Ok(GenerationPreparation::ConfigurationFailure {
            error: configuration_error(
                "configured_tools_unavailable",
                "One or more configured tools are unavailable in this process".to_string(),
                serde_json::json!({ "tools": missing }),
            ),
        });
    }
    let tools: Vec<crate::ai::types::tool::Tool> = active_tool_names
        .iter()
        .filter_map(|name| tools_by_name.get(name.as_str()).copied().cloned())
        .collect();

    let messages = match read_bounded_context(lane, drive, generation).await? {
        ContinueOperationResult::CancelRequested => {
            return Ok(GenerationPreparation::CancelRequested);
        }
        ContinueOperationResult::Result { value } => value,
    };
    let system_prompt = resolve_system_prompt(lane);
    let before_request = lane.hooks().run_with_gate(
        crate::agent_core::harness::hooks::HookInvocation {
            lane: lane.name().to_string(),
            run_id: drive.operation_id().to_string(),
            event: crate::agent_core::harness::hooks::HookEvent::BeforeRequest(
                crate::agent_core::harness::hooks::BeforeRequestEvent {
                    model: model.clone(),
                    step: crate::agent_core::harness::hooks::RequestStep::Assistant,
                    attempt: next_attempt,
                    stream_options: gen_stream_options.clone(),
                },
            ),
        },
        Arc::new(drive.gate().clone()),
        drive.context().clone(),
    );
    let before_request = before_request.await?;
    let stream_options = match &before_request {
        crate::agent_core::harness::hooks::HookResult::BeforeRequest(Some(result)) => {
            crate::agent_core::harness::hooks::apply_stream_options_patch(
                gen_stream_options,
                &result.stream_options,
            )
        }
        _ => gen_stream_options,
    };
    Ok(GenerationPreparation::Ready(PreparedGeneration {
        model,
        tools,
        messages,
        system_prompt,
        to_provider_messages: config.to_provider_messages.clone().unwrap_or_else(|| {
            Arc::new(|messages, _context| {
                Box::pin(
                    async move { crate::agent_core::harness::messages::convert_to_llm(&messages) },
                )
            })
        }),
        stream_options,
    }))
}

pub async fn publish_generation_intent(
    lane: &Arc<Lane>,
    drive: &Drive,
    ready: &OperationState,
    prepared: &PreparedGeneration,
) -> anyhow::Result<ContinueOperationResult<OperationState>> {
    let OperationPhase::AssistantReady {
        generation_context,
        next_attempt,
    } = &ready.phase
    else {
        anyhow::bail!("Generation intent requires an assistant-ready operation");
    };
    let at = crate::ai::now_ms();
    let pending = OperationPhase::AssistantEffectPending {
        generation_context: generation_context.clone(),
        attempt: *next_attempt,
        response_entry_id: lane.session().id_generator().next(Some(at)),
        usage_id: lane.session().id_generator().next(Some(at)),
        intended_output_limit: prepared.model.max_tokens,
        context_window: prepared.model.context_window,
    };
    let first_attempt = *next_attempt == 1;
    let turn_id = generation_context.step_id.clone();
    let lane_name = lane.name().to_owned();
    let run_id = drive.operation_id().to_owned();
    lane.continue_operation(
        move |_state, current, _meta, _reader| {
            // The durable scope can change after preparation (e.g. termination
            // latch); never overwrite it with the caller's stale ready snapshot.
            let intent = OperationState {
                scope: current.scope.clone(),
                phase: pending.clone(),
            };
            let event = HarnessEvent::TurnStart {
                lane: lane_name.clone(),
                run_id: run_id.clone(),
                turn_id: turn_id.clone(),
                recovery: false,
            };
            Box::pin(async move {
                Ok(OperationCommand::Commit {
                    writes: Vec::new(),
                    operation_state: intent.clone(),
                    lane: None,
                    materialize: Box::new(move |_| intent),
                    events: Some(Box::new(move |_| {
                        Ok(if first_attempt {
                            vec![event]
                        } else {
                            Vec::new()
                        })
                    })),
                })
            })
        },
        drive.context().clone(),
    )
    .await
}

/// Execute a published intent with the real harness lifecycle and gated hooks.
/// The provider receives the options produced by execution/assistant, with a
/// lane-scoped session id and the drive's abort signal overriding caller state.
pub async fn perform_generation(
    lane: &Arc<Lane>,
    drive: &Drive,
    intent: &OperationState,
    prepared: &PreparedGeneration,
) -> anyhow::Result<AssistantMessage> {
    let OperationPhase::AssistantEffectPending {
        generation_context,
        response_entry_id,
        ..
    } = &intent.phase
    else {
        anyhow::bail!("Generation execution requires an assistant-effect-pending operation");
    };
    let response = Arc::new(AssistantResponseLifecycle::open(
        Arc::clone(lane),
        drive,
        response_entry_id,
        false,
    ));
    let transform_lane = Arc::clone(lane);
    let transform_gate = drive.gate().clone();
    let transform_run = drive.operation_id().to_owned();
    let payload_lane = Arc::clone(lane);
    let payload_gate = drive.gate().clone();
    let payload_run = drive.operation_id().to_owned();
    let after = Arc::clone(&response);
    let after_gate = drive.gate().clone();
    let request_lane = Arc::clone(lane);
    let request_gate = drive.gate().clone();
    let request_model = prepared.model.clone();
    let config = HarnessAssistantStreamConfig {
        model: prepared.model.clone(),
        system_prompt: prepared.system_prompt.clone(),
        tools: Some(prepared.tools.clone()),
        thinking_level: generation_context.configuration.thinking_level,
        stream_options: prepared.stream_options.clone(),
        transform_context: Some(Arc::new(move |request: HarnessRequestContext, context| {
            let lane = Arc::clone(&transform_lane);
            let gate = transform_gate.clone();
            let run_id = transform_run.clone();
            Box::pin(async move {
                let result = lane
                    .hooks()
                    .run_with_gate(
                        HookInvocation {
                            lane: lane.name().to_owned(),
                            run_id,
                            event: HookEvent::TransformContext(TransformContextEvent {
                                messages: request.messages.clone(),
                                system_prompt: request.system_prompt.clone(),
                            }),
                        },
                        Arc::new(gate),
                        context,
                    )
                    .await?;
                Ok(match result {
                    HookResult::TransformContext(Some(patch)) => HarnessRequestContext {
                        messages: patch.messages.unwrap_or(request.messages),
                        system_prompt: patch.system_prompt.unwrap_or(request.system_prompt),
                    },
                    _ => request,
                })
            })
        })),
        to_provider_messages: Arc::clone(&prepared.to_provider_messages),
        before_payload: Some(Arc::new(move |payload, model, context| {
            let lane = Arc::clone(&payload_lane);
            let gate = payload_gate.clone();
            let run_id = payload_run.clone();
            Box::pin(async move {
                let result = lane
                    .hooks()
                    .run_with_gate(
                        HookInvocation {
                            lane: lane.name().to_owned(),
                            run_id,
                            event: HookEvent::BeforePayload(BeforePayloadEvent { payload, model }),
                        },
                        Arc::new(gate),
                        context,
                    )
                    .await?;
                Ok(match result {
                    HookResult::BeforePayload(Some(result)) => Some(result.payload),
                    _ => None,
                })
            })
        })),
        after_response: Some(Arc::new(move |message, metadata, context| {
            let response = Arc::clone(&after);
            let gate = after_gate.clone();
            Box::pin(async move {
                response
                    .after_response(&metadata, &message, Arc::new(gate), context)
                    .await
            })
        })),
        request: Arc::new(move |ai_context, mut options, context| {
            let lane = Arc::clone(&request_lane);
            let gate = request_gate.clone();
            let model = request_model.clone();
            Box::pin(async move {
                let admitted = with_abort_signal(gate.signal(), context.clone());
                options.simple.stream.session_id =
                    Some(format!("{}:{}", lane.session().metadata().id, lane.name()));
                options.simple.stream.signal = admitted.abort_signal();
                options.simple.stream.callbacks.on_payload = options.on_payload.map(|callback| {
                    Arc::new(move |payload, model| callback(payload, model, context.clone()))
                        as Arc<crate::ai::types::request_callbacks::PayloadHook>
                });
                options.simple.stream.callbacks.on_response = options.on_response.map(|callback| {
                    Arc::new(
                        move |metadata: crate::ai::types::request_callbacks::ProviderResponse,
                              _model| {
                            let callback = Arc::clone(&callback);
                            Box::pin(async move {
                                callback(assistant::AssistantResponseMetadata {
                                    status: Some(metadata.status),
                                    headers: Some(metadata.headers),
                                });
                                Ok(())
                            })
                                as futures::future::BoxFuture<'static, anyhow::Result<()>>
                        },
                    ) as Arc<crate::ai::types::request_callbacks::ResponseHook>
                });
                Ok(gate.admit(|| {
                    lane.models().stream_simple(
                        &model,
                        &ai_context,
                        Some(crate::ai::models::ModelsSimpleStreamOptions {
                            simple: options.simple,
                            transform_headers: None,
                        }),
                    )
                })?)
            })
        }),
        observer: Arc::new(LifecycleObserver(Arc::clone(&response))),
    };
    // Rust equivalent of try/finally, including transform/request/observer errors.
    let result =
        assistant::stream_harness_assistant(&prepared.messages, &config, drive.context().clone())
            .await;
    response.close().await?;
    result
}

pub async fn run_retry_wait(
    lane: &Arc<Lane>,
    drive: &Drive,
    generation: &OperationState,
) -> anyhow::Result<PublishOutcome> {
    let (not_before, gc, na, sid) = match &generation.phase {
        OperationPhase::AssistantRetryWait {
            generation_context,
            retry,
        } => (
            retry.not_before,
            generation_context.clone(),
            retry.next_attempt,
            generation_context.step_id.clone(),
        ),
        _ => anyhow::bail!("runRetryWait requires AssistantRetryWait"),
    };
    if crate::ai::now_ms() < not_before && !drive.wait_for_retry() {
        return Ok(PublishOutcome::Waiting {
            outcome: crate::agent_core::harness::runtime::drive_pass::DriveOutcome::Waiting {
                operation_id: drive.operation_id().to_string(),
                reason: WaitingReason::Retry { not_before },
            },
        });
    }
    if crate::ai::now_ms() < not_before && drive.wait_for_retry() {
        let reason: Arc<dyn std::error::Error + Send + Sync> =
            Arc::new(std::io::Error::other("retry wait aborted"));
        let waiting = drive
            .gate()
            .admit(|| wait_until(not_before, drive.gate().signal(), reason))?;
        if let Err(error) = waiting.await {
            // Preserve the gate's typed abort/close refusal, not a stringified
            // timer error, when cancellation wins during the wait.
            drive.gate().admit(|| ())?;
            return Err(error);
        }
    }
    let pl = Arc::clone(lane);
    let retry_run_id = drive.operation_id().to_owned();
    let gc2 = gc.clone();
    let na2 = na;
    let sid2 = sid.clone();
    let result = lane
        .continue_operation(
            move |_state, current, _meta, _reader| {
                let gc = gc2.clone();
                let ln = pl.clone();
                let na = na2;
                let sid = sid2.clone();
                let run_id = retry_run_id.clone();
                Box::pin(async move {
                    let ns = OperationState {
                        scope: current.scope.clone(),
                        phase: OperationPhase::AssistantReady {
                            generation_context: gc,
                            next_attempt: na,
                        },
                    };
                    Ok(OperationCommand::Commit {
                        writes: Vec::new(),
                        operation_state: ns,
                        lane: None,
                        materialize: Box::new(|_c| PublishOutcome::Continue),
                        events: Some(Box::new(move |_c: &CommitResult| {
                            Ok(vec![HarnessEvent::RetryStart {
                                lane: ln.name().to_string(),
                                run_id,
                                step: sid,
                                attempt: na,
                            }])
                        })),
                    })
                })
            },
            drive.context().clone(),
        )
        .await?;
    Ok(match result {
        ContinueOperationResult::CancelRequested => PublishOutcome::Continue,
        ContinueOperationResult::Result { value } => value,
    })
}

pub async fn run_generation(
    lane: &Arc<Lane>,
    drive: &Drive,
    generation: &OperationState,
) -> anyhow::Result<PublishOutcome> {
    if matches!(&generation.phase, OperationPhase::AssistantRetryWait { .. }) {
        return run_retry_wait(lane, drive, generation).await;
    }
    let prepared = match prepare_generation(lane, drive, generation).await? {
        GenerationPreparation::Ready(p) => p,
        GenerationPreparation::ConfigurationFailure { error } => {
            return publish_configuration_failure(lane, drive, generation, error).await;
        }
        GenerationPreparation::CancelRequested => return Ok(PublishOutcome::Continue),
    };
    let intent = match publish_generation_intent(lane, drive, generation, &prepared).await? {
        ContinueOperationResult::CancelRequested => return Ok(PublishOutcome::Continue),
        ContinueOperationResult::Result { value } => value,
    };
    let response = perform_generation(lane, drive, &intent, &prepared).await?;
    Ok(
        match publish_response(lane, drive, &intent, &response, false).await? {
            super::publish::PublishOutcome::Continue => ProcedureResult::Continue,
            super::publish::PublishOutcome::Settled { outcome } => {
                ProcedureResult::Settled { outcome: *outcome }
            }
        },
    )
}

use super::publish::publish_response;

#[cfg(test)]
mod tests;
