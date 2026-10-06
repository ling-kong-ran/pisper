//! Port of `drive/response.ts` `publishResponse` (`response.ts:182-485`):
//! classify one assistant-generation or deferred-poll response and settle it
//! atomically — overflow recovery into a summary decision, deferred
//! suspension, bounded retry waits, tool-call dispatch, terminal checkpoints,
//! failure cleanup, and the event batch.
//!
//! Organization note: upstream keeps this beside the response helpers; the
//! Rust port splits it into `response.rs` (helpers + lifecycle) and this
//! module (the settlement classifier).

use std::sync::Arc;

use crate::agent_core::harness::runtime::drive::response::{
    normalize_aborted, normalize_error, provider_error, uuid_v7_timestamp, ResponseSource,
};
use crate::agent_core::harness::runtime::drive::retry::{retry_not_before, RetryDelayPolicy};
use crate::agent_core::harness::runtime::drive::structural::prepare_overflow_compaction;
use crate::agent_core::harness::runtime::drive::terminal::{
    operation_cleanup_writes, operation_result_record,
};
use crate::agent_core::harness::runtime::durable::{
    CheckpointData, Continuation, DeferredScope, OperationPhase, OperationState, RetryWait,
    SummaryReason, SummaryTask, ToolBatch, ToolCall as DurableToolCall, ToolCallState,
};
use crate::agent_core::harness::runtime::events::{HarnessEvent, RunEndStatus};
use crate::agent_core::harness::runtime::lane::{Lane, LanePatch, OperationCommand};
use crate::agent_core::harness::session::commit::{insert_entry, insert_usage};
use crate::agent_core::harness::session::types::{
    CommitResult, Control, Entry, NewEntry, NewUsageRow, OperationError, TerminalStatus, Write,
};
use crate::agent_core::harness::session::values::{
    branch_tip, delete_list, operation_preparation, pending_assistant_frames, set_value,
};
use crate::agent_core::harness::session::Session as _;
use crate::agent_core::types::AgentMessage;
use crate::ai::overflow::{is_context_overflow, is_recoverable_length};
use crate::ai::retry::is_retryable_assistant_error;
use crate::ai::types::message::{AssistantBlock, AssistantMessage};
use crate::ai::types::primitives::StopReason;

/// The procedure result vocabulary (`runtime/types.ts` `ProcedureResult`):
/// `Continue` forwards control, `Settled` carries the terminal record.
/// Box the rare terminal record so ordered JSON payloads do not enlarge every
/// continuing/checkpoint state; this internal enum has no separate wire shape.
#[derive(Debug, Clone, PartialEq)]
pub enum PublishOutcome {
    Continue,
    Settled {
        outcome: Box<crate::agent_core::harness::session::OperationResultRecord>,
    },
}

fn checkpoint_state(
    scope: &crate::agent_core::harness::runtime::durable::OperationScope,
    trigger_entry_id: String,
) -> OperationState {
    OperationState {
        scope: scope.clone(),
        phase: OperationPhase::Checkpoint {
            checkpoint: CheckpointData {
                continuation: Continuation::MayFinish {
                    include_final_assistant: true,
                },
                trigger_entry_id,
            },
        },
    }
}

fn summary_deciding_state(
    scope: &crate::agent_core::harness::runtime::durable::OperationScope,
    task_id: String,
    trigger_entry_id: String,
) -> OperationState {
    OperationState {
        scope: scope.clone(),
        phase: OperationPhase::SummaryDeciding {
            task: SummaryTask {
                task_id,
                reason: Some(SummaryReason::Overflow),
                custom_instructions: None,
                boundary:
                    crate::agent_core::harness::runtime::durable::ResultBoundary::ResumeCheckpoint {
                        resume_after: CheckpointData {
                            continuation: Continuation::NeedAssistant {
                                overflow_recovery_used: true,
                            },
                            trigger_entry_id,
                        },
                    },
            },
        },
    }
}

fn retry_delays(
    retry: &crate::agent_core::harness::runtime::durable::NormalizedRetryPolicy,
    attempt: u32,
) -> (i64, i64) {
    let policy = RetryDelayPolicy {
        base_delay_ms: retry.base_delay_ms as i64,
        max_agent_delay_ms: Some(retry.max_agent_delay_ms as i64),
    };
    let delay_ms = crate::ai::retry::retry_delay_ms(
        &crate::ai::retry::RetryPolicy {
            enabled: true,
            max_retries: u32::MAX,
            base_delay_ms: retry.base_delay_ms,
            max_agent_delay_ms: Some(retry.max_agent_delay_ms),
        },
        attempt,
    ) as i64;
    let not_before = retry_not_before(&policy, attempt as i64, crate::ai::now_ms());
    (delay_ms, not_before)
}

fn deferred_scope_from_assistant(
    current: &OperationState,
    response_entry_id: &str,
) -> Option<DeferredScope> {
    let OperationPhase::AssistantEffectPending {
        generation_context, ..
    } = &current.phase
    else {
        return None;
    };
    Some(DeferredScope {
        step_id: generation_context.step_id.clone(),
        source_entry_id: response_entry_id.to_string(),
        poll: 0,
        configuration: generation_context.configuration.clone(),
        stream_options: generation_context.stream_options.clone(),
    })
}

/// Upstream `publishResponse` (`response.ts:182-485`).
#[allow(clippy::too_many_lines)]
pub async fn publish_response(
    lane: &Arc<Lane>,
    drive: &crate::agent_core::harness::runtime::drive_pass::Drive,
    intent: &OperationState,
    response: &AssistantMessage,
    recovery: bool,
) -> anyhow::Result<PublishOutcome> {
    let context = drive.context().clone();
    let operation_id = drive.operation_id().to_string();
    // Phase-scoped ids and limits (`response.ts:199-207`).
    let (response_entry_id, usage_id, context_window, intended_output_limit) = match &intent.phase {
        OperationPhase::AssistantEffectPending {
            response_entry_id,
            usage_id,
            intended_output_limit,
            context_window,
            ..
        } => (
            response_entry_id.clone(),
            usage_id.clone(),
            Some(*context_window),
            Some(*intended_output_limit),
        ),
        OperationPhase::DeferredEffectPending {
            response_entry_id,
            usage_id,
            ..
        } => (response_entry_id.clone(), usage_id.clone(), None, None),
        _ => anyhow::bail!("publishResponse requires an effect-pending operation"),
    };
    let assistant_effect_pending =
        matches!(&intent.phase, OperationPhase::AssistantEffectPending { .. });
    let overflow = assistant_effect_pending
        && (is_context_overflow(response, context_window)
            || is_recoverable_length(response, intended_output_limit.unwrap_or(0)));
    let overflow_recovery_used = matches!(
        &intent.phase,
        OperationPhase::AssistantEffectPending {
            generation_context,
            ..
        } if generation_context.overflow_recovery_used
    );
    let overflow_preparation = if overflow && !overflow_recovery_used {
        prepare_overflow_compaction(lane, drive, intent).await?
    } else {
        None
    };

    let response = response.clone();
    let planner_lane = Arc::clone(lane);
    let planner_context = context.clone();
    // Upstream `publishResponse` runs its planner behind `lane.settleOperation`
    // (`response.ts:196`), not `continueOperation`: a cancelled durable control
    // is settled BY this procedure (the planner's own `cancel_requested` branch
    // normalizes the response and advances to checkpoint), so the
    // cancel-short-circuit of `continueOperation` would skip it.
    let result = lane
        .settle_operation(
            move |state, current, meta, reader| {
                let lane = planner_lane.clone();
                let context = planner_context.clone();
                let response = response.clone();
                let overflow_preparation = overflow_preparation.clone();
                let operation_id = operation_id.clone();
                let response_entry_id = response_entry_id.clone();
                let usage_id = usage_id.clone();
                let context = context.clone();
                let source_tip_id = meta.source_tip_id.clone();
                Box::pin(async move {
                    // Classification against the CURRENT durable state
                    // (`response.ts:199-320`).
                    let source = if matches!(
                        &current.phase,
                        OperationPhase::AssistantEffectPending { .. }
                    ) {
                        ResponseSource::Assistant
                    } else {
                        ResponseSource::Deferred
                    };
                    let (configuration, turn_id, attempt, retry_policy, deferred_scope) =
                        match &current.phase {
                            OperationPhase::AssistantEffectPending {
                                generation_context,
                                attempt,
                                ..
                            } => (
                                Some(generation_context.configuration.clone()),
                                generation_context.step_id.clone(),
                                Some(*attempt),
                                Some(generation_context.retry_policy.clone()),
                                None,
                            ),
                            OperationPhase::DeferredEffectPending {
                                deferred, ..
                            } => (
                                Some(deferred.configuration.clone()),
                                format!("{}:poll:{}", deferred.step_id, deferred.poll),
                                None,
                                None,
                                Some(deferred.clone()),
                            ),
                            _ => anyhow::bail!(
                                "publishResponse requires an effect-pending operation"
                            ),
                        };
                    let configuration = configuration
                        .ok_or_else(|| anyhow::anyhow!("publishResponse requires a configuration"))?;
                    let mut response_scope = current.scope.clone();
                    response_scope.latest_assistant_entry_id = Some(response_entry_id.clone());
                    let mut committed = response.clone();
                    let mut settled: Option<OperationState> = None;
                    let mut failure: Option<OperationError> = None;

                    let cancel_requested =
                        matches!(current.scope.control, Control::CancelRequested { .. });
                    if cancel_requested {
                        committed = normalize_aborted(source, &response);
                        settled = Some(checkpoint_state(
                            &response_scope,
                            response_entry_id.clone(),
                        ));
                    } else if response.stop_reason == StopReason::Aborted {
                        anyhow::bail!(
                            "{} response is aborted while durable control is running",
                            source.label()
                        );
                    } else if assistant_effect_pending && overflow {
                        committed = normalize_error(
                            &response,
                            response
                                .error_message
                                .as_deref()
                                .unwrap_or("Assistant request exceeded the context window"),
                        );
                        let preparation = overflow_preparation
                            .as_ref()
                            .filter(|_| !overflow_recovery_used);
                        match preparation {
                            Some(preparation) => {
                                let trigger_entry_id = match &current.phase {
                                    OperationPhase::AssistantEffectPending {
                                        generation_context,
                                        ..
                                    } => generation_context.trigger_entry_id.clone(),
                                    _ => anyhow::bail!("Unreachable overflow branch"),
                                };
                                settled = Some(summary_deciding_state(
                                    &response_scope,
                                    preparation.task_id.clone(),
                                    trigger_entry_id,
                                ));
                            }
                            None => {
                                failure = Some(provider_error(source, &committed));
                            }
                        }
                    } else if response.stop_reason == StopReason::Deferred {
                        if assistant_effect_pending {
                            if super::response::deferred_handle_is_valid(
                                &response,
                                &configuration.model,
                            ) {
                                let scope = deferred_scope_from_assistant(
                                    current,
                                    &response_entry_id,
                                )
                                .expect("assistant branch");
                                settled = Some(OperationState {
                                    scope: response_scope.clone(),
                                    phase: OperationPhase::DeferredSuspended {
                                        deferred: scope,
                                    },
                                });
                            } else {
                                committed = normalize_error(
                                    &response,
                                    "Provider returned an invalid deferred handle",
                                );
                                failure = Some(provider_error(source, &committed));
                            }
                        } else {
                            let mut scope = deferred_scope
                                .clone()
                                .expect("deferred branch carries its scope");
                            scope.source_entry_id = response_entry_id.clone();
                            settled = Some(OperationState {
                                scope: response_scope.clone(),
                                phase: OperationPhase::DeferredSuspended {
                                    deferred: scope,
                                },
                            });
                        }
                    } else if response.stop_reason == StopReason::Error {
                        let attempt_number = attempt.unwrap_or(1);
                        let retry_eligible = assistant_effect_pending
                            && (recovery || is_retryable_assistant_error(&response));
                        let under_max = match &retry_policy {
                            Some(policy) => attempt_number < policy.max_attempts,
                            None => false,
                        };
                        if assistant_effect_pending && retry_eligible && under_max {
                            let generation_context = match &current.phase {
                                OperationPhase::AssistantEffectPending {
                                    generation_context,
                                    ..
                                } => generation_context.clone(),
                                _ => anyhow::bail!("Unreachable retry branch"),
                            };
                            let policy = retry_policy
                                .as_ref()
                                .expect("assistant branch carries a retry policy");
                            let not_before = retry_not_before(
                                &RetryDelayPolicy {
                                    base_delay_ms: policy.base_delay_ms as i64,
                                    max_agent_delay_ms: Some(policy.max_agent_delay_ms as i64),
                                },
                                attempt_number as i64,
                                crate::ai::now_ms(),
                            );
                            settled = Some(OperationState {
                                scope: response_scope.clone(),
                                phase: OperationPhase::AssistantRetryWait {
                                    generation_context,
                                    retry: RetryWait {
                                        next_attempt: attempt_number + 1,
                                        not_before,
                                        error_message: response
                                            .error_message
                                            .clone()
                                            .unwrap_or_else(|| {
                                                "Assistant request failed".to_string()
                                            }),
                                    },
                                },
                            });
                        } else {
                            failure = Some(provider_error(source, &response));
                        }
                    } else {
                        let call_indices: Vec<usize> = response
                            .content
                            .iter()
                            .enumerate()
                            .filter_map(|(index, block)| {
                                matches!(block, AssistantBlock::ToolCall(_)).then_some(index)
                            })
                            .collect();
                        if !call_indices.is_empty() {
                            let timestamp = uuid_v7_timestamp(&response_entry_id)?;
                            let planned: Vec<DurableToolCall> = call_indices
                                .iter()
                                .map(|source_index| DurableToolCall {
                                    source_index: *source_index,
                                    result_entry_id: lane
                                        .session()
                                        .id_generator()
                                        .next(Some(timestamp)),
                                    state: ToolCallState::Planned,
                                })
                                .collect();
                            settled = Some(OperationState {
                                scope: response_scope.clone(),
                                phase: OperationPhase::Tools {
                                    batch: ToolBatch {
                                        assistant_entry_id: response_entry_id.clone(),
                                        configuration,
                                        turn_id: turn_id.clone(),
                                        calls: planned,
                                    },
                                },
                            });
                        } else if response.stop_reason == StopReason::ToolUse {
                            committed = normalize_error(
                                &response,
                                "Provider reported tool use without any tool calls",
                            );
                            failure = Some(provider_error(source, &committed));
                        } else {
                            settled = Some(checkpoint_state(
                                &response_scope,
                                response_entry_id.clone(),
                            ));
                        }
                    }
                    if settled.is_none() && failure.is_none() {
                        anyhow::bail!("Response settlement has no durable disposition");
                    }

                    // Settlement writes (`response.ts:322-356`).
                    let entry = NewEntry::Message {
                        id: response_entry_id.clone(),
                        parent_id: state.tip_id.clone(),
                        message: AgentMessage::Assistant(committed.clone()),
                        terminate: None,
                    };
                    let new_usage = NewUsageRow {
                        id: usage_id.clone(),
                        usage: committed.usage,
                        entry_id: Some(response_entry_id.clone()),
                        adjustment: false,
                        details: None,
                    };
                    let mut writes: Vec<Write> = vec![
                        insert_entry(entry.clone()),
                        insert_usage(new_usage.clone()),
                        set_value(
                            &branch_tip(lane.name()),
                            serde_json::Value::String(response_entry_id.clone()),
                        ),
                    ];
                    if failure.is_none() {
                        writes.push(delete_list(&pending_assistant_frames(
                            &operation_id,
                            &response_entry_id,
                        )));
                    } else {
                        let cleanup = operation_cleanup_writes(
                            reader,
                            &operation_id,
                            current,
                            context.clone(),
                        )
                        .await?;
                        writes.extend(cleanup);
                    }
                    if let (
                        Some(operations_state),
                        Some(preparation),
                    ) = (&settled, &overflow_preparation)
                    {
                        if matches!(
                            &operations_state.phase,
                            OperationPhase::SummaryDeciding { .. }
                        ) {
                            writes.push(set_value(
                                &operation_preparation(&operation_id, &preparation.task_id),
                                serde_json::to_value(&preparation.preparation)
                                    .expect("preparation serializes"),
                            ));
                        }
                    }

                    // Event batch builder (`response.ts:362-462`): entry_added,
                    // usage, retry/turn/compaction/run_suspend side events, and
                    // run_end on failure — shared by the finish and commit paths.
                    let settled_for_events = settled.clone();
                    let committed_for_events = committed.clone();
                    let lane_name = lane.name().to_string();
                    let run_id = operation_id.clone();
                    let response_entry_for_events = response_entry_id.clone();
                    let usage_for_events = new_usage.clone();
                    let entry_for_events = entry.clone();
                    let deferred_current = !assistant_effect_pending;
                    let record_failure_for_events = failure.clone();
                    let record_ended_at =
                        failure.as_ref().map(|_| crate::ai::now_ms()).unwrap_or_default();
                    let retry_policy_for_events = retry_policy.clone();
                    let source_tip_id_for_events = source_tip_id.clone();
                    let attempt = attempt.unwrap_or(1);
                    let events = move |commit: &CommitResult| {
                        let Some(entry_seq) = commit.seqs.first().copied() else {
                            anyhow::bail!("commit sequence missing for response entry");
                        };
                        let Some(usage_seq) = commit.seqs.get(1).copied() else {
                            anyhow::bail!("commit sequence missing for usage row");
                        };
                        let entry = match &entry_for_events {
                            NewEntry::Message {
                                id,
                                parent_id,
                                message,
                                terminate,
                            } => Entry::Message {
                                id: id.clone(),
                                parent_id: parent_id.clone(),
                                seq: entry_seq,
                                timestamp: commit.timestamp,
                                message: message.clone(),
                                terminate: *terminate,
                            },
                            _ => anyhow::bail!("publishResponse only materializes messages"),
                        };
                        let row = crate::agent_core::harness::session::types::UsageRow {
                            id: usage_for_events.id.clone(),
                            seq: usage_seq,
                            usage: usage_for_events.usage,
                            entry_id: usage_for_events.entry_id.clone(),
                            adjustment: usage_for_events.adjustment,
                            details: usage_for_events.details.clone(),
                        };
                        let mut events = vec![
                            HarnessEvent::EntryAdded {
                                lane: lane_name.clone(),
                                entry,
                            },
                            HarnessEvent::Usage {
                                lane: lane_name.clone(),
                                row,
                                totals: commit.stats.usage,
                            },
                        ];
                        let settled_wait = matches!(
                            &settled_for_events,
                            Some(OperationState {
                                phase: OperationPhase::AssistantRetryWait { .. },
                                ..
                            })
                        );
                        let settled_tools = matches!(
                            &settled_for_events,
                            Some(OperationState {
                                phase: OperationPhase::Tools { .. },
                                ..
                            })
                        );
                        let settled_summary = matches!(
                            &settled_for_events,
                            Some(OperationState {
                                phase: OperationPhase::SummaryDeciding { .. },
                                ..
                            })
                        );
                        let settled_suspended = matches!(
                            &settled_for_events,
                            Some(OperationState {
                                phase: OperationPhase::DeferredSuspended { .. },
                                ..
                            })
                        );
                        if assistant_effect_pending {
                            // retry_end (`response.ts:374-390`).
                            if !recovery && attempt > 1 && !settled_wait {
                                let success = committed_for_events.stop_reason
                                    != StopReason::Error
                                    && committed_for_events.stop_reason
                                        != StopReason::Aborted;
                                let final_error = if success {
                                    None
                                } else {
                                    Some(
                                        committed_for_events
                                            .error_message
                                            .clone()
                                            .unwrap_or_else(|| {
                                                format!(
                                                    "Assistant request ended with {:?}",
                                                    committed_for_events.stop_reason
                                                )
                                            }),
                                    )
                                };
                                events.push(HarnessEvent::RetryEnd {
                                    lane: lane_name.clone(),
                                    run_id: run_id.clone(),
                                    step: turn_id.clone(),
                                    attempt,
                                    success,
                                    final_error,
                                });
                            }
                            // retry_scheduled (`response.ts:391-403`).
                            if !recovery && settled_wait {
                                let (next_attempt, not_before, error_message) =
                                    match &settled_for_events {
                                        Some(OperationState {
                                            phase: OperationPhase::AssistantRetryWait {
                                                retry,
                                                ..
                                            },
                                            ..
                                        }) => (
                                            retry.next_attempt,
                                            retry.not_before,
                                            retry.error_message.clone(),
                                        ),
                                        _ => unreachable!("settled_wait checked"),
                                    };
                                let max_attempts = match &settled_for_events {
                                    Some(OperationState {
                                        phase: OperationPhase::AssistantRetryWait {
                                            generation_context,
                                            ..
                                        },
                                        ..
                                    }) => generation_context.retry_policy.max_attempts,
                                    _ => unreachable!("settled_wait checked"),
                                };
                                let retry_policy_for_events = retry_policy_for_events
                                    .as_ref()
                                    .expect("assistant branch carries a retry policy");
                                let (delay_ms, _) =
                                    retry_delays(retry_policy_for_events, attempt);
                                events.push(HarnessEvent::RetryScheduled {
                                    lane: lane_name.clone(),
                                    run_id: run_id.clone(),
                                    step: turn_id.clone(),
                                    attempt: next_attempt,
                                    max_attempts,
                                    delay_ms,
                                    not_before,
                                    error_message,
                                });
                            }
                            // turn_end (`response.ts:404-413`).
                            if !recovery && !settled_tools && !settled_wait {
                                events.push(HarnessEvent::TurnEnd {
                                    lane: lane_name.clone(),
                                    run_id: run_id.clone(),
                                    turn_id: turn_id.clone(),
                                    message: AgentMessage::Assistant(
                                    committed_for_events.clone(),
                                ),
                                    tool_results: Vec::new(),
                                    recovery: false,
                                });
                            }
                            // compaction_start (`response.ts:414-422`).
                            if settled_summary {
                                events.push(HarnessEvent::CompactionStart {
                                    lane: lane_name.clone(),
                                    run_id: run_id.clone(),
                                    reason: crate::agent_core::harness::runtime::durable::SummaryReason::Overflow,
                                    started_at: commit.timestamp,
                                });
                            }
                        } else if !settled_tools {
                            // Deferred-side turn_end (`response.ts:423-433`).
                            events.push(HarnessEvent::TurnEnd {
                                lane: lane_name.clone(),
                                run_id: run_id.clone(),
                                turn_id: turn_id.clone(),
                                message: AgentMessage::Assistant(
                                    committed_for_events.clone(),
                                ),
                                tool_results: Vec::new(),
                                recovery,
                            });
                        }
                        // run_suspend (`response.ts:434-448`).
                        let suspend_due =
                            (deferred_current || !recovery) && settled_suspended;
                        if suspend_due {
                            if let Some(deferred) = &committed_for_events.deferred {
                                events.push(HarnessEvent::RunSuspend {
                                    lane: lane_name.clone(),
                                    run_id: run_id.clone(),
                                    deferred: deferred.clone(),
                                    poll: suspended_poll(&settled_for_events),
                                    recovery: recovery && deferred_current,
                                });
                            }
                        }
                        // run_end failed (`response.ts:449-460`).
                        if let Some(failure) = &record_failure_for_events {
                            events.push(HarnessEvent::RunEnd {
                                lane: lane_name.clone(),
                                run_id: run_id.clone(),
                                status: RunEndStatus::Failed,
                                error: Some(failure.clone()),
                                from_tip_id: source_tip_id_for_events.clone(),
                                tip_id: Some(response_entry_for_events.clone()),
                                ended_at: record_ended_at,
                            });
                        }
                        Ok(events)
                    };

                    match &failure {
                        Some(_) => {
                            let record = operation_result_record(
                                meta,
                                TerminalStatus::Failed,
                                Some(response_entry_id.clone()),
                                Some(failure.clone().expect("failed arm carries an error")),
                            )?;
                            let outcome = record.clone();
                            Ok(OperationCommand::Finish {
                                writes,
                                record,
                                lane: Some(LanePatch {
                                    tip_id: Some(response_entry_id.clone()),
                                    configuration: None,
                                    inbox: None,
                                }),
                                materialize: Box::new(move |_commit| {
                                    PublishOutcome::Settled { outcome: Box::new(outcome) }
                                }),
                                events: Some(Box::new(events)),
                            })
                        }
                        None => {
                            let Some(operation_state) = settled else {
                                anyhow::bail!("Response settlement is missing its next state");
                            };
                            Ok(OperationCommand::Commit {
                                writes,
                                operation_state,
                                lane: Some(LanePatch {
                                    tip_id: Some(response_entry_id.clone()),
                                    configuration: None,
                                    inbox: None,
                                }),
                                materialize: Box::new(|_commit| PublishOutcome::Continue),
                                events: Some(Box::new(events)),
                            })
                        }
                    }
                })
            },
            context,
        )
        .await?;
    Ok(result)
}

fn suspended_poll(settled: &Option<OperationState>) -> u64 {
    match settled {
        Some(OperationState {
            phase: OperationPhase::DeferredSuspended { deferred },
            ..
        }) => deferred.poll,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core::harness::context::background_context;
    use crate::agent_core::harness::runtime::drive::recovery::interrupted_assistant_message;
    use crate::agent_core::harness::runtime::lane::Lane;

    async fn lane_for_invariant_checks() -> std::sync::Arc<Lane> {
        use crate::agent_core::harness::runtime::lane::RuntimeConfig;
        use crate::agent_core::harness::runtime::restore::restore_lane;
        use crate::agent_core::harness::session::LaneConfiguration;
        use crate::agent_core::harness::session::{
            LaneModel, MemoryStorage, MemoryStorageOptions, SessionMetadata, StorageBackedSession,
        };
        use crate::agent_core::harness::DEFAULT_COMPACTION_SETTINGS;
        use crate::agent_core::types::{QueueMode, ThinkingLevel, ToolExecutionMode};
        use crate::ai::models::faux::{faux_provider, FauxProviderOptions};
        use crate::ai::models::{create_models, CreateModelsOptions};
        let storage = MemoryStorage::new(MemoryStorageOptions::default());
        let sess = std::sync::Arc::new(StorageBackedSession::new(
            SessionMetadata {
                id: "publish-invariant-test".into(),
                created_at: 1,
                storage_version: 1,
                ..Default::default()
            },
            std::sync::Arc::new(storage),
        ));
        let writes: Vec<crate::agent_core::harness::session::types::Write> = vec![
            crate::agent_core::harness::session::values::set_value(
                &crate::agent_core::harness::session::values::branch_tip("main"),
                serde_json::Value::Null,
            ),
            crate::agent_core::harness::session::values::set_value(
                &crate::agent_core::harness::session::values::lane_config("main"),
                crate::agent_core::harness::session::values::lane_configuration_value(
                    &LaneConfiguration {
                        model: LaneModel {
                            provider: "faux".to_string(),
                            model_id: "faux-1".to_string(),
                        },
                        thinking_level: ThinkingLevel::Off,
                        active_tool_names: Vec::new(),
                    },
                ),
            ),
            crate::agent_core::harness::session::values::set_value(
                &crate::agent_core::harness::session::values::lane_state("main"),
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
        models.set_provider(std::sync::Arc::clone(&faux.provider));
        let state = restore_lane(sess.as_ref(), "main", background_context())
            .await
            .expect("lane restores");
        let emit: crate::agent_core::harness::runtime::lane::EmitBatch =
            std::sync::Arc::new(|_events, _context| Box::pin(async { Ok(()) }));
        Lane::new(
            "main",
            sess,
            models,
            crate::agent_core::harness::hooks::HookRegistry::new(std::sync::Arc::new(
                |_error: anyhow::Error,
                 _hook: crate::agent_core::harness::hooks::HookName,
                 _message: String,
                 _context| { Box::pin(async {}) },
            )),
            state,
            std::sync::Arc::new(|error: anyhow::Error| error),
            emit,
            std::sync::Arc::new(move || RuntimeConfig {
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

    #[test]
    fn interrupted_message_without_partial_uses_the_warning() {
        let message = interrupted_assistant_message("faux", "faux-1", None);
        assert_eq!(message.stop_reason, StopReason::Error);
        assert!(message
            .error_message
            .as_deref()
            .unwrap()
            .starts_with("Assistant request was interrupted."));
        assert_eq!(message.usage.input, 0);
        assert_eq!(message.usage.total_tokens, 0);
        assert_eq!(message.provider, "faux");
        assert_eq!(message.api, "unknown");
    }

    #[test]
    fn interrupted_message_keeps_partial_content_with_zeroed_usage() {
        let partial: AssistantMessage = serde_json::from_str(
            r#"{"role":"assistant","content":[{"type":"text","text":"partial"}],"api":"faux","provider":"faux","model":"faux-1","stopReason":"stop","usage":{"input":7,"output":9,"cacheRead":0,"cacheWrite":0,"totalTokens":16,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"timestamp":3}"#,
        )
        .unwrap();
        let settled = interrupted_assistant_message("faux", "faux-1", Some(&partial));
        assert_eq!(settled.stop_reason, StopReason::Error);
        assert_eq!(settled.usage.input, 0);
        assert_eq!(settled.usage.output, 0);
    }

    #[tokio::test]
    async fn publish_response_requires_a_running_operation() {
        let lane = lane_for_invariant_checks().await;
        let drive = crate::agent_core::harness::runtime::drive_pass::Drive::new(
            &crate::agent_core::harness::runtime::drive_pass::DriveOptions {
                operation_id: "op1".to_string(),
                wait_for_retry: None,
                poll_deferred: None,
            },
            background_context(),
        );
        let intent: OperationState = serde_json::from_value(serde_json::json!({
            "control": {"status": "running"},
            "settings": {"compaction": {"enabled": true, "reserveTokens": 16384,
                "keepRecentTokens": 20000},
                "steeringMode": "all", "followUpMode": "all", "toolExecution": "parallel"},
            "latestAssistantEntryId": null,
            "at": "assistant.effect_pending",
            "generationContext": {
                "stepId": "s1", "triggerEntryId": "t1",
                "configuration": {"model": {"provider": "faux", "modelId": "faux-1"},
                    "thinkingLevel": "off", "activeToolNames": []},
                "streamOptions": {},
                "retryPolicy": {"maxAttempts": 4, "baseDelayMs": 1000,
                    "maxAgentDelayMs": 60000},
                "overflowRecoveryUsed": false
            },
            "attempt": 1,
            "responseEntryId": "r1",
            "usageId": "u1",
            "intendedOutputLimit": 1000,
            "contextWindow": 100000
        }))
        .unwrap();
        let response: AssistantMessage = serde_json::from_str(
            r#"{"role":"assistant","content":[],"api":"faux","provider":"faux","model":"faux-1","stopReason":"stop","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"timestamp":9}"#,
        )
        .unwrap();
        let error = publish_response(&lane, &drive, &intent, &response, false)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("no operation to settle"),
            "unexpected error: {error}"
        );
    }
}
