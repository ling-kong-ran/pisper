//! Port of `drive/recovery.ts` (126 lines): settle orphaned assistant
//! requests from their committed frame prefixes without another provider
//! call.

use std::sync::Arc;

use super::publish::{publish_response, PublishOutcome};
use crate::agent_core::harness::runtime::drive_pass::{Drive, ProcedureResult};
use crate::agent_core::harness::runtime::durable::{OperationPhase, OperationState};
use crate::agent_core::harness::runtime::events::HarnessEvent;
use crate::agent_core::harness::runtime::lane::{ContinueOperationResult, Lane, OperationCommand};
use crate::agent_core::harness::runtime::progress::read_assistant_frames;
use crate::agent_core::types::AgentMessage;
use crate::ai::frame::reduce_assistant_message_frames;
use crate::ai::types::message::AssistantMessage;
use crate::ai::types::primitives::StopReason;

/// Upstream `ZERO_USAGE` (`recovery.ts:13-20`).
fn zero_usage() -> serde_json::Value {
    serde_json::json!({
        "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0,
        "totalTokens": 0,
        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0}
    })
}

/// Upstream `interruptedAssistantMessage` (`recovery.ts:22-41`): the fixed
/// interruption warning plus zeroed usage over the committed frame prefix.
pub(crate) fn interrupted_assistant_message(
    provider: &str,
    model_id: &str,
    partial: Option<&AssistantMessage>,
) -> AssistantMessage {
    const WARNING: &str = "Assistant request was interrupted. The preceding content is the latest committed partial; newer live output may be missing and the external outcome is unknown.";
    match partial {
        None => serde_json::from_value(serde_json::json!({
            "role": "assistant",
            "content": [],
            "api": "unknown",
            "provider": provider,
            "model": model_id,
            "usage": zero_usage(),
            "stopReason": "error",
            "errorMessage": WARNING,
            "timestamp": crate::ai::now_ms(),
        }))
        .expect("interrupted message literal parses"),
        Some(partial) => {
            let mut settled = partial.clone();
            settled.usage =
                serde_json::from_value(zero_usage()).expect("zero usage literal parses");
            settled.stop_reason = StopReason::Error;
            settled.error_message = Some(WARNING.to_string());
            settled
        }
    }
}

/// The configured model identity behind one effect-pending operation
/// (`recovery.ts:100-103` reads it from either phase shape).
fn configured_provider_model(effect: &OperationState) -> Option<(String, String)> {
    match &effect.phase {
        OperationPhase::AssistantEffectPending {
            generation_context, ..
        } => Some((
            generation_context.configuration.model.provider.clone(),
            generation_context.configuration.model.model_id.clone(),
        )),
        OperationPhase::DeferredEffectPending { deferred, .. } => Some((
            deferred.configuration.model.provider.clone(),
            deferred.configuration.model.model_id.clone(),
        )),
        _ => None,
    }
}

pub(crate) fn response_entry_id_of(effect: &OperationState) -> Option<String> {
    match &effect.phase {
        OperationPhase::AssistantEffectPending {
            response_entry_id, ..
        }
        | OperationPhase::DeferredEffectPending {
            response_entry_id, ..
        } => Some(response_entry_id.clone()),
        _ => None,
    }
}

/// Upstream `recoverAssistantGeneration` (`recovery.ts:44-84`): settle an
/// orphaned assistant generation from its bounded committed frame prefix.
pub async fn recover_assistant_generation(
    lane: &Arc<Lane>,
    drive: &Drive,
    generation: &OperationState,
) -> anyhow::Result<ProcedureResult> {
    let Some(response_entry_id) = response_entry_id_of(generation) else {
        anyhow::bail!("Assistant generation recovery requires the matching phase");
    };
    let context = drive.context().clone();
    let operation_id = drive.operation_id().to_string();
    let closure_entry_id = response_entry_id.clone();
    let frames = lane
        .continue_operation(
            move |_state, _current, _meta, reader| {
                let response_entry_id = closure_entry_id.clone();
                let operation_id = operation_id.clone();
                let context = context.clone();
                Box::pin(async move {
                    let frames =
                        read_assistant_frames(reader, &operation_id, &response_entry_id, context)
                            .await?;
                    Ok(OperationCommand::Return { result: frames })
                })
            },
            drive.context().clone(),
        )
        .await?;
    let raw_frames = match frames {
        ContinueOperationResult::Result { value } => value,
        ContinueOperationResult::CancelRequested => return Ok(ProcedureResult::Continue),
    };
    // The staged channel stores serialized frames; rehydrate them typed.
    let frames = raw_frames
        .iter()
        .map(|value: &serde_json::Value| {
            serde_json::from_value::<crate::ai::frame::AssistantMessageFrame>(value.clone())
        })
        .collect::<Result<Vec<crate::ai::frame::AssistantMessageFrame>, _>>()?;
    let (provider, model_id) = configured_provider_model(generation).ok_or_else(|| {
        anyhow::anyhow!("Assistant generation recovery requires the matching phase")
    })?;
    let message = interrupted_assistant_message(
        &provider,
        &model_id,
        reduce_assistant_message_frames(&frames)?.as_ref(),
    );
    emit_interrupted(lane, drive, &message, &response_entry_id).await?;
    match publish_response(lane, drive, generation, &message, true).await? {
        PublishOutcome::Continue => Ok(ProcedureResult::Continue),
        PublishOutcome::Settled { outcome } => Ok(ProcedureResult::Settled { outcome: *outcome }),
    }
}

/// Upstream `recoverCancelledAssistantEffect` (`recovery.ts:87-126`):
/// synthetically settle one cancelled orphaned assistant or deferred effect
/// under its reserved ids.
pub async fn recover_cancelled_assistant_effect(
    lane: &Arc<Lane>,
    drive: &Drive,
    effect: &OperationState,
) -> anyhow::Result<ProcedureResult> {
    let Some(response_entry_id) = response_entry_id_of(effect) else {
        anyhow::bail!("Effect recovery requires an effect-pending phase");
    };
    let context = drive.context().clone();
    let operation_id = drive.operation_id().to_string();
    let closure_entry_id = response_entry_id.clone();
    // Upstream `recoverCancelledAssistantEffect` reads the staged frames
    // behind `lane.settleOperation` (`recovery.ts:88-97`), not
    // `continueOperation`: the read must run even though the cancelled
    // control is the very state this recovery settles, so the
    // cancel-short-circuit of `continueOperation` would deadlock the
    // reconciliation loop. (`recoverAssistantGeneration` keeps
    // `continueOperation`, matching upstream.)
    let frames = lane
        .settle_operation(
            move |_state, _current, _meta, reader| {
                let response_entry_id = closure_entry_id.clone();
                let operation_id = operation_id.clone();
                let context = context.clone();
                Box::pin(async move {
                    let frames =
                        read_assistant_frames(reader, &operation_id, &response_entry_id, context)
                            .await?;
                    Ok(OperationCommand::Return { result: frames })
                })
            },
            drive.context().clone(),
        )
        .await?;
    let frames = frames
        .iter()
        .map(|value: &serde_json::Value| {
            serde_json::from_value::<crate::ai::frame::AssistantMessageFrame>(value.clone())
        })
        .collect::<Result<Vec<crate::ai::frame::AssistantMessageFrame>, _>>()?;
    let (provider, model_id) = configured_provider_model(effect)
        .ok_or_else(|| anyhow::anyhow!("Effect recovery requires the matching phase"))?;
    let message = interrupted_assistant_message(
        &provider,
        &model_id,
        reduce_assistant_message_frames(&frames)?.as_ref(),
    );
    emit_interrupted(lane, drive, &message, &response_entry_id).await?;
    match publish_response(lane, drive, effect, &message, true).await? {
        PublishOutcome::Continue => Ok(ProcedureResult::Continue),
        PublishOutcome::Settled { outcome } => Ok(ProcedureResult::Settled { outcome: *outcome }),
    }
}

/// The upstream `lane.emitBatch([...])` pair
/// (`recovery.ts:63-82`, `:105-124`): message_start/message_end with the
/// recovery flag.
async fn emit_interrupted(
    lane: &Arc<Lane>,
    drive: &Drive,
    message: &AssistantMessage,
    response_entry_id: &str,
) -> anyhow::Result<()> {
    let lane_name = lane.name().to_string();
    let run_id = drive.operation_id().to_string();
    let message = AgentMessage::Assistant(message.clone());
    lane.emit_batch(
        vec![
            HarnessEvent::MessageStart {
                recovery: true,
                lane: lane_name.clone(),
                run_id: Some(run_id.clone()),
                message: message.clone(),
            },
            HarnessEvent::MessageEnd {
                recovery: true,
                lane: lane_name,
                run_id: Some(run_id),
                message,
                entry_id: response_entry_id.to_string(),
            },
        ],
        drive.context().clone(),
    )
    .await
}
