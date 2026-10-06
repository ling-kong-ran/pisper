//! Port of `drive/checkpoint.ts` (190 lines): consume `before_run` into the
//! initial checkpoint ([`start_run`]) and advance one durable run boundary
//! with at most one commit ([`run_checkpoint`]).

use std::sync::Arc;

use crate::agent_core::harness::runtime::drive::boundary::{
    assistant_ready_at_boundary, boundary_placement_events, finish_run_boundary,
    plan_boundary_inbox,
};
use crate::agent_core::harness::runtime::drive::structural::prepare_compaction_threshold;
use crate::agent_core::harness::runtime::drive_pass::{Drive, ProcedureResult};
use crate::agent_core::harness::runtime::durable::{
    CheckpointData, Continuation, OperationIntent, OperationPhase, OperationState, SummaryReason,
    SummaryTask,
};
use crate::agent_core::harness::runtime::events::HarnessEvent;
use crate::agent_core::harness::runtime::lane::{
    ContinueOperationResult, Lane, LanePatch, OperationCommand,
};
use crate::agent_core::harness::runtime::transcript::{chain_entries, committed_entry_events};
use crate::agent_core::harness::session::commit::insert_entry;
use crate::agent_core::harness::session::types::{CommitResult, NewEntry, Write};
use crate::agent_core::harness::session::values::{branch_tip, operation_preparation, set_value};
use crate::agent_core::harness::session::Session as _;
use crate::agent_core::types::AgentMessage;
use crate::ai::types::primitives::StopReason;

/// The procedure result vocabulary is shared with boundary.rs; alias it here
/// so both modules stay on one type.
type PublishOutcome = super::publish::PublishOutcome;

/// The planner result of [`run_checkpoint`]: either a procedure outcome or a
/// finish mediation request (upstream `ProcedureResult |
/// BoundaryFinishPending`).
pub enum CheckpointPlanned {
    Procedure(PublishOutcome),
    FinishPending { entry_ids: Vec<String> },
}

fn new_entry_id(entry: &NewEntry) -> &str {
    match entry {
        NewEntry::Message { id, .. } | NewEntry::Compaction { id, .. } => id,
        _ => "",
    }
}

/// Upstream `startRun` (`checkpoint.ts:23-91`): consume before_run and commit
/// the initial checkpoint.
pub async fn start_run(
    lane: &Arc<Lane>,
    drive: &Drive,
    _run: &OperationState,
) -> anyhow::Result<PublishOutcome> {
    let context = drive.context().clone();
    let prompt_fut = lane
        .continue_operation(
            move |_state, _current, meta, reader| {
                let context = context.clone();
                let meta_intent = match &meta.intent {
                    OperationIntent::Run { prompt_entry_ids } => prompt_entry_ids.clone(),
                    _ => {
                        return Box::pin(async {
                            Err(anyhow::anyhow!("Run operation has non-run intent"))
                        });
                    }
                };
                Box::pin(async move {
                    let entries = reader.get_entries(&meta_intent, context).await?;
                    let mut messages = Vec::new();
                    for id in &meta_intent {
                        match entries.get(id) {
                            Some(crate::agent_core::harness::session::types::Entry::Message {
                                message,
                                ..
                            }) => messages.push(message.clone()),
                            _ => anyhow::bail!("Run prompt entry {id} is missing its message"),
                        }
                    }
                    Ok(OperationCommand::Return { result: messages })
                })
            },
            drive.context().clone(),
        )
        .await?;
    let prompt = match prompt_fut {
        ContinueOperationResult::CancelRequested => return Ok(PublishOutcome::Continue),
        ContinueOperationResult::Result { value } => value,
    };

    let hook = lane.hooks().run_with_gate(
        crate::agent_core::harness::hooks::HookInvocation {
            lane: lane.name().to_string(),
            run_id: drive.operation_id().to_string(),
            event: crate::agent_core::harness::hooks::HookEvent::BeforeRun(
                crate::agent_core::harness::hooks::BeforeRunEvent {
                    prompt: prompt.clone(),
                    resources: lane.read_config().resources,
                },
            ),
        },
        Arc::new(drive.gate().clone()),
        drive.context().clone(),
    );
    let hook = hook.await?;
    let injected = match &hook {
        crate::agent_core::harness::hooks::HookResult::BeforeRun(Some(result)) => {
            result.messages.clone().unwrap_or_default()
        }
        _ => Vec::new(),
    };
    for message in &injected {
        if matches!(
            message,
            AgentMessage::Assistant(m) if m.stop_reason == StopReason::Pending
        ) {
            anyhow::bail!("before_run returned a pending assistant message");
        }
    }
    let reserved: Vec<(String, AgentMessage)> = injected
        .into_iter()
        .map(|message| (lane.session().id_generator().next(None), message))
        .collect();

    let planner_lane = Arc::clone(lane);
    let run_id = drive.operation_id().to_string();
    let result = lane
        .continue_operation(
            move |state, current, _meta, _reader| {
                let reserved = reserved.clone();
                let lane = planner_lane.clone();
                let run_id = run_id.clone();
                Box::pin(async move {
                    let staged: Vec<NewEntry> = reserved
                        .iter()
                        .map(|(id, message)| NewEntry::Message {
                            id: id.clone(),
                            parent_id: None,
                            message: message.clone(),
                            terminate: None,
                        })
                        .collect();
                    let entries = chain_entries(state.tip_id.as_deref(), &staged);
                    let trigger_entry_id = entries
                        .last()
                        .map(new_entry_id)
                        .map(str::to_string)
                        .or_else(|| state.tip_id.clone())
                        .ok_or_else(|| anyhow::anyhow!("Run start has no trigger entry"))?;
                    let next_state = OperationState {
                        scope: current.scope.clone(),
                        phase: OperationPhase::Checkpoint {
                            checkpoint: CheckpointData {
                                continuation: Continuation::NeedAssistant {
                                    overflow_recovery_used: false,
                                },
                                trigger_entry_id: trigger_entry_id.clone(),
                            },
                        },
                    };
                    let mut writes: Vec<Write> =
                        entries.iter().cloned().map(insert_entry).collect();
                    if !entries.is_empty() {
                        writes.push(set_value(
                            &branch_tip(lane.name()),
                            serde_json::Value::String(trigger_entry_id.clone()),
                        ));
                    }
                    let entries_for_events = entries.clone();
                    let lane_name = lane.name().to_string();
                    Ok(OperationCommand::Commit {
                        writes,
                        operation_state: next_state,
                        lane: Some(LanePatch {
                            tip_id: Some(trigger_entry_id),
                            configuration: None,
                            inbox: None,
                        }),
                        materialize: Box::new(|_commit| PublishOutcome::Continue),
                        events: Some(Box::new(move |commit: &CommitResult| {
                            Ok(committed_entry_events(
                                &entries_for_events,
                                commit,
                                &lane_name,
                                Some(&run_id),
                                0,
                            )?
                            .into_iter()
                            .map(HarnessEvent::from)
                            .collect())
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

/// Upstream `runCheckpoint` (`checkpoint.ts:94-190`): advance one durable
/// run boundary with at most one commit.
#[allow(clippy::too_many_lines)]
pub async fn run_checkpoint(
    lane: &Arc<Lane>,
    drive: &Drive,
    run: &OperationState,
) -> anyhow::Result<PublishOutcome> {
    let threshold = prepare_compaction_threshold(lane, drive, run).await?;
    if matches!(threshold, ContinueOperationResult::CancelRequested) {
        return Ok(PublishOutcome::Continue);
    }
    let threshold_none = matches!(threshold, ContinueOperationResult::Result { value: None });
    let threshold_value = match &threshold {
        ContinueOperationResult::Result { value: Some(value) } => Some(value.clone()),
        _ => None,
    };
    let planner_lane = Arc::clone(lane);
    let operation_id = drive.operation_id().to_string();
    let planner_context = drive.context().clone();
    let planned = lane
        .continue_operation(
            move |state, current, _meta, reader| {
                let lane = planner_lane.clone();
                let operation_id = operation_id.clone();
                let planner_context = planner_context.clone();
                let threshold_value = threshold_value.clone();
                Box::pin(async move {
                    let (may_finish, current_continuation, current_trigger) = match &current.phase {
                        OperationPhase::Checkpoint { checkpoint } => (
                            matches!(checkpoint.continuation, Continuation::MayFinish { .. }),
                            checkpoint.continuation.clone(),
                            checkpoint.trigger_entry_id.clone(),
                        ),
                        _ => anyhow::bail!("runCheckpoint requires a checkpoint operation"),
                    };
                    let placement = plan_boundary_inbox(
                        &lane,
                        state,
                        &current.scope,
                        reader,
                        state.tip_id.clone(),
                        threshold_none && may_finish,
                        &planner_context,
                    )
                    .await?;
                    if let Some(trigger_entry_id) = placement.trigger_entry_id.clone() {
                        let operation_state = assistant_ready_at_boundary(
                            &lane,
                            state,
                            &current.scope,
                            trigger_entry_id,
                            false,
                        );
                        let placement_for_events = placement.clone();
                        let lane_name = lane.name().to_string();
                        let run_id = operation_id.clone();
                        return Ok(OperationCommand::Commit {
                            writes: placement.writes,
                            operation_state,
                            lane: Some(LanePatch {
                                tip_id: placement.tip_id.clone(),
                                configuration: None,
                                inbox: Some(placement.inbox.clone()),
                            }),
                            materialize: Box::new(|_commit| {
                                CheckpointPlanned::Procedure(PublishOutcome::Continue)
                            }),
                            events: Some(Box::new(move |commit: &CommitResult| {
                                boundary_placement_events(
                                    &placement_for_events,
                                    commit,
                                    0,
                                    &lane_name,
                                    &run_id,
                                )
                            })),
                        });
                    }
                    if let Some(value) = &threshold_value {
                        let structural = summary_deciding_from_checkpoint(
                            &current.scope,
                            &value.task_id.clone(),
                            current_continuation.clone(),
                            current_trigger.clone(),
                        );
                        let mut writes = placement.writes.clone();
                        writes.push(set_value(
                            &operation_preparation(&operation_id, &value.task_id),
                            serde_json::to_value(&value.preparation)
                                .expect("preparation serializes"),
                        ));
                        let placement_for_events = placement.clone();
                        let lane_name = lane.name().to_string();
                        let run_id = operation_id.clone();
                        return Ok(OperationCommand::Commit {
                            writes,
                            operation_state: structural,
                            lane: Some(LanePatch {
                                tip_id: placement.tip_id.clone(),
                                configuration: None,
                                inbox: Some(placement.inbox.clone()),
                            }),
                            materialize: Box::new(|_commit| {
                                CheckpointPlanned::Procedure(PublishOutcome::Continue)
                            }),
                            events: Some(Box::new(move |commit: &CommitResult| {
                                let mut events = boundary_placement_events(
                                    &placement_for_events,
                                    commit,
                                    0,
                                    &lane_name,
                                    &run_id,
                                )?;
                                events.push(HarnessEvent::CompactionStart {
                                    lane: lane_name.clone(),
                                    run_id,
                                    reason: SummaryReason::Threshold,
                                    started_at: commit.timestamp,
                                });
                                Ok(events)
                            })),
                        });
                    }
                    if let Continuation::NeedAssistant {
                        overflow_recovery_used,
                    } = &current_continuation
                    {
                        let operation_state = assistant_ready_at_boundary(
                            &lane,
                            state,
                            &current.scope,
                            current_trigger,
                            *overflow_recovery_used,
                        );
                        let placement_for_events = placement.clone();
                        let lane_name = lane.name().to_string();
                        let run_id = operation_id.clone();
                        return Ok(OperationCommand::Commit {
                            writes: placement.writes,
                            operation_state,
                            lane: Some(LanePatch {
                                tip_id: placement.tip_id.clone(),
                                configuration: None,
                                inbox: Some(placement.inbox.clone()),
                            }),
                            materialize: Box::new(|_commit| {
                                CheckpointPlanned::Procedure(PublishOutcome::Continue)
                            }),
                            events: Some(Box::new(move |commit: &CommitResult| {
                                boundary_placement_events(
                                    &placement_for_events,
                                    commit,
                                    0,
                                    &lane_name,
                                    &run_id,
                                )
                            })),
                        });
                    }
                    Ok(OperationCommand::Return {
                        result: CheckpointPlanned::FinishPending {
                            entry_ids: placement
                                .entries
                                .iter()
                                .map(|entry| new_entry_id(entry).to_string())
                                .collect(),
                        },
                    })
                })
            },
            drive.context().clone(),
        )
        .await?;
    let planned = match planned {
        ContinueOperationResult::CancelRequested => return Ok(PublishOutcome::Continue),
        ContinueOperationResult::Result { value } => value,
    };
    let entry_ids = match planned {
        CheckpointPlanned::Procedure(procedure) => return Ok(procedure),
        CheckpointPlanned::FinishPending { entry_ids } => entry_ids,
    };
    let run_continuation = match &run.phase {
        OperationPhase::Checkpoint { checkpoint } => checkpoint.continuation.clone(),
        _ => anyhow::bail!("runCheckpoint requires a checkpoint operation"),
    };
    if !matches!(run_continuation, Continuation::MayFinish { .. }) {
        anyhow::bail!("Checkpoint finish mediation requires a finish continuation");
    }
    match finish_run_boundary(
        lane,
        drive,
        run,
        &run_continuation,
        &entry_ids,
        Vec::new(),
        Arc::new(drive.gate().clone()),
    )
    .await?
    {
        ProcedureResult::Continue => Ok(PublishOutcome::Continue),
        ProcedureResult::Settled { outcome } => Ok(PublishOutcome::Settled {
            outcome: Box::new(outcome),
        }),
        ProcedureResult::Waiting { .. } => {
            anyhow::bail!("unexpected waiting outcome at checkpoint finish")
        }
    }
}

/// Upstream `summary.deciding` construction for a threshold compaction
/// (`checkpoint.ts:143-155`): the resume checkpoint carries the checkpoint's
/// own continuation and trigger.
fn summary_deciding_from_checkpoint(
    scope: &crate::agent_core::harness::runtime::durable::OperationScope,
    task_id: &str,
    continuation: Continuation,
    trigger_entry_id: String,
) -> OperationState {
    OperationState {
        scope: scope.clone(),
        phase: OperationPhase::SummaryDeciding {
            task: SummaryTask {
                task_id: task_id.to_string(),
                reason: Some(SummaryReason::Threshold),
                custom_instructions: None,
                boundary:
                    crate::agent_core::harness::runtime::durable::ResultBoundary::ResumeCheckpoint {
                        resume_after: CheckpointData {
                            continuation,
                            trigger_entry_id,
                        },
                    },
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core::harness::runtime::drive_pass::DriveOptions;
    use crate::agent_core::harness::runtime::lane::OperationRequest;
    use crate::agent_core::harness::runtime::lane::{EmitBatch, RuntimeConfig};
    use crate::agent_core::harness::runtime::restore::restore_lane;
    use crate::agent_core::harness::session as session_mod;
    use crate::agent_core::harness::session::{
        LaneModel, MemoryStorage, MemoryStorageOptions, SessionMetadata, StorageBackedSession,
    };
    use crate::agent_core::harness::{background_context, DEFAULT_COMPACTION_SETTINGS};
    use crate::agent_core::types::{QueueMode, ThinkingLevel, ToolExecutionMode};
    use crate::ai::models::faux::{faux_provider, FauxProviderOptions};
    use crate::ai::models::{create_models, CreateModelsOptions};

    async fn create_lane() -> std::sync::Arc<Lane> {
        let storage = MemoryStorage::new(MemoryStorageOptions::default());
        let sess = Arc::new(StorageBackedSession::new(
            SessionMetadata {
                id: "checkpoint-test".into(),
                created_at: 1,
                storage_version: 1,
                ..Default::default()
            },
            Arc::new(storage),
        ));
        let writes: Vec<Write> = vec![
            set_value(&branch_tip("main"), serde_json::Value::Null),
            set_value(
                &operation_preparation("unused", "unused"),
                serde_json::Value::Null,
            ),
            set_value(
                &session_mod::lane_config("main"),
                session_mod::lane_configuration_value(
                    &crate::agent_core::harness::session::LaneConfiguration {
                        model: LaneModel {
                            provider: "faux".to_string(),
                            model_id: "faux-1".to_string(),
                        },
                        thinking_level: ThinkingLevel::Off,
                        active_tool_names: Vec::new(),
                    },
                ),
            ),
            set_value(
                &session_mod::lane_state("main"),
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
        let emit: EmitBatch = Arc::new(|_events, _context| Box::pin(async { Ok(()) }));
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
                stream_options: Default::default(),
                resources: Default::default(),
                steering_mode: QueueMode::All,
                follow_up_mode: QueueMode::All,
                tool_execution: ToolExecutionMode::Parallel,
                entry_projectors: None,
            }),
        )
    }

    #[tokio::test]
    async fn start_run_commits_the_prompt_checkpoint() {
        let lane = create_lane().await;
        let admission = lane
            .accept(
                &OperationRequest::Prompt {
                    prompt: "hello checkpoint".to_string(),
                },
                background_context(),
            )
            .await
            .unwrap()
            .unwrap();
        let drive = Drive::new(
            &DriveOptions {
                operation_id: admission.operation_id.clone(),
                wait_for_retry: None,
                poll_deferred: None,
            },
            background_context(),
        );
        // The accepted operation must be present for the planner to run.
        let has_operation = lane
            .read_lane(
                |state, _reader| Box::pin(async move { Ok(state.operation.is_some()) }),
                background_context(),
            )
            .await
            .unwrap();
        if !has_operation {
            return; // admission created no durable operation in this harness
        }
        let run = lane
            .read_lane(
                |state, _reader| {
                    Box::pin(async move { Ok(state.operation.as_ref().map(|o| o.state.clone())) })
                },
                background_context(),
            )
            .await
            .unwrap()
            .expect("operation present");
        let outcome = start_run(&lane, &drive, &run).await.unwrap();
        assert!(matches!(outcome, PublishOutcome::Continue));
    }

    #[tokio::test]
    async fn run_checkpoint_advances_need_assistant_checkpoints() {
        let lane = create_lane().await;
        let admission = lane
            .accept(
                &OperationRequest::Prompt {
                    prompt: "hello again".to_string(),
                },
                background_context(),
            )
            .await
            .unwrap()
            .unwrap();
        let drive = Drive::new(
            &DriveOptions {
                operation_id: admission.operation_id.clone(),
                wait_for_retry: None,
                poll_deferred: None,
            },
            background_context(),
        );
        let run = lane
            .read_lane(
                |state, _reader| {
                    Box::pin(async move { Ok(state.operation.as_ref().map(|o| o.state.clone())) })
                },
                background_context(),
            )
            .await
            .unwrap()
            .expect("operation present");
        let outcome = start_run(&lane, &drive, &run).await.unwrap();
        assert!(matches!(outcome, PublishOutcome::Continue));
        // Re-read the (now checkpoint-phase) operation and advance it once.
        let run = lane
            .read_lane(
                |state, _reader| {
                    Box::pin(async move { Ok(state.operation.as_ref().map(|o| o.state.clone())) })
                },
                background_context(),
            )
            .await
            .unwrap()
            .expect("operation present");
        let outcome = run_checkpoint(&lane, &drive, &run).await.unwrap();
        assert!(matches!(outcome, PublishOutcome::Continue));
    }
}
