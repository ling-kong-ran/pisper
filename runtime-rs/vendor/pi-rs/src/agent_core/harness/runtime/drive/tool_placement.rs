//! Port of `packages/agent/src/harness/runtime/drive/tool-placement.ts`
//! (294 lines): reading a tool batch's assistant source, staging-check
//! placement reads, the durable placement commit, and the emit orchestration
//! that materializes settled tool results into entries and events.
//!
//! Substitutions: upstream `Map<number, AgentToolCall>` is a `HashMap`; the
//! staged-result duck check (`value.type === "message"` +
//! `isToolResultMessage`) inspects the stored JSON payload's `role` field.

use std::collections::HashMap;

use crate::agent_core::harness::runtime::drive_pass::Drive;
use crate::agent_core::harness::runtime::durable::{
    CheckpointData, Continuation, OperationPhase, OperationState, ToolBatch, ToolCall,
    ToolCallState,
};
use crate::agent_core::harness::runtime::events::HarnessEvent;
use crate::agent_core::harness::runtime::lane::{Lane, LaneCommand, LanePatch, OperationCommand};
use crate::agent_core::harness::session::commit::{insert_entry, insert_usage};
use crate::agent_core::harness::session::types::{Entry, NewEntry, NewUsageRow, UsageRow, Write};
use crate::agent_core::harness::session::values::{
    branch_tip, delete_value, operation_tool_args_prefix, pending_entry, set_value,
};
use crate::agent_core::harness::session::Session as _;
use crate::agent_core::types::{AgentMessage, AgentToolCall};
use crate::ai::types::message::{AssistantMessage, ToolResultMessage};

/// Upstream `ToolBatchSource`.
#[derive(Debug, Clone)]
pub struct ToolBatchSource {
    pub assistant: AssistantMessage,
    pub calls: HashMap<usize, AgentToolCall>,
}

/// Upstream `readToolBatchSource` (`tool-placement.ts:27-49`): validate the
/// batch's assistant entry and the source index of every planned call.
pub async fn read_tool_batch_source(
    lane: &Lane,
    drive: &Drive,
    batch: &ToolBatch,
) -> anyhow::Result<ToolBatchSource> {
    let context = drive.context().clone();
    let call_context = context.clone();
    let batch = batch.clone();
    lane.command(
        move |_state, reader| {
            let context = context.clone();
            let batch = batch.clone();
            Box::pin(async move {
                let entry = reader
                    .get_entries(
                        std::slice::from_ref(&batch.assistant_entry_id),
                        context.clone(),
                    )
                    .await?
                    .get(&batch.assistant_entry_id)
                    .cloned();
                let (assistant_message, content) = match &entry {
                    Some(Entry::Message {
                        message: AgentMessage::Assistant(message),
                        ..
                    }) => (message.clone(), message.content.clone()),
                    _ => anyhow::bail!("Tool batch assistant entry is invalid"),
                };
                let mut calls = HashMap::new();
                for call in &batch.calls {
                    let block = content.get(call.source_index);
                    let Some(crate::ai::types::message::AssistantBlock::ToolCall(block)) = block
                    else {
                        anyhow::bail!(
                            "Tool call source index {} does not name a tool-call block",
                            call.source_index
                        );
                    };
                    calls.insert(call.source_index, block.clone());
                }
                Ok(LaneCommand::Return {
                    result: ToolBatchSource {
                        assistant: assistant_message,
                        calls,
                    },
                })
            })
        },
        call_context,
    )
    .await
}

/// Upstream `toolCallFor` (`tool-placement.ts:65-71`).
pub fn tool_call_for<'a>(
    sources: &'a ToolBatchSource,
    call: &ToolCall,
) -> anyhow::Result<&'a AgentToolCall> {
    sources
        .calls
        .get(&call.source_index)
        .ok_or_else(|| anyhow::anyhow!("Tool call source index {} is invalid", call.source_index))
}

/// Upstream `withToolBatch` (`tool-placement.ts:73-75`): the same operation
/// scope carrying an updated tool batch.
pub fn with_tool_batch(run: &OperationState, batch: ToolBatch) -> OperationState {
    OperationState {
        scope: run.scope.clone(),
        phase: OperationPhase::Tools { batch },
    }
}

/// Upstream `PlacementItem`.
#[derive(Debug, Clone)]
struct PlacementItem {
    call: ToolCall,
    message: ToolResultMessage,
}

/// Upstream `PlacementRead`.
#[derive(Debug, Clone)]
struct PlacementRead {
    items: Vec<PlacementItem>,
    turn_results: Option<Vec<ToolResultMessage>>,
}

fn staged_tool_result_payload(payload: &serde_json::Value) -> Option<ToolResultMessage> {
    if payload.get("role").and_then(|role| role.as_str()) == Some("toolResult") {
        serde_json::from_value(payload.clone()).ok()
    } else {
        None
    }
}

fn staged_tool_result_message(message: &AgentMessage) -> Option<ToolResultMessage> {
    match message {
        AgentMessage::ToolResult(message) => Some(message.clone()),
        _ => None,
    }
}

/// Upstream `readPlacement` (`tool-placement.ts:77-135`): the contiguous
/// `outcome_ready` prefix, staged results validated against their sources,
/// plus the completed-batch turn results when every call resolves.
async fn read_placement(
    lane: &Lane,
    drive: &Drive,
    sources: &ToolBatchSource,
) -> anyhow::Result<Option<PlacementRead>> {
    let context = drive.context().clone();
    let call_context = context.clone();
    let sources = sources.clone();
    lane.command(
        move |state, reader| {
            let context = context.clone();
            let sources = sources.clone();
            Box::pin(async move {
                let Some(operation) = &state.operation else {
                    return Ok(LaneCommand::Return { result: None });
                };
                let OperationPhase::Tools { batch: current } = &operation.state.phase else {
                    return Ok(LaneCommand::Return { result: None });
                };
                let Some(first) = current
                    .calls
                    .iter()
                    .position(|call| !matches!(call.state, ToolCallState::Completed { .. }))
                else {
                    return Ok(LaneCommand::Return { result: None });
                };
                let mut ready = Vec::new();
                let mut index = first;
                while index < current.calls.len() {
                    let call = &current.calls[index];
                    if !matches!(call.state, ToolCallState::OutcomeReady { .. }) {
                        break;
                    }
                    ready.push(call.clone());
                    index += 1;
                }
                if ready.is_empty() {
                    return Ok(LaneCommand::Return { result: None });
                }

                let mut items = Vec::new();
                for call in &ready {
                    let stored = reader
                        .get_value(&pending_entry(&call.result_entry_id), context.clone())
                        .await?;
                    let Some(message) = stored
                        .as_ref()
                        .and_then(|stored| staged_tool_result_payload(&stored.value))
                    else {
                        anyhow::bail!(
                            "Tool call {} is missing its staged result",
                            call.result_entry_id
                        );
                    };
                    let source = tool_call_for(&sources, call)?;
                    if message.tool_call_id != source.id || message.tool_name != source.name {
                        anyhow::bail!(
                            "Tool call {} has a mismatched staged result",
                            call.result_entry_id
                        );
                    }
                    items.push(PlacementItem {
                        call: call.clone(),
                        message,
                    });
                }

                let mut turn_results = None;
                if index == current.calls.len() {
                    let placed_ids: Vec<String> = current
                        .calls
                        .iter()
                        .filter(|call| matches!(call.state, ToolCallState::Completed { .. }))
                        .map(|call| call.result_entry_id.clone())
                        .collect();
                    let placed = reader.get_entries(&placed_ids, context.clone()).await?;
                    let staged: HashMap<&str, &ToolResultMessage> = items
                        .iter()
                        .map(|item| (item.call.result_entry_id.as_str(), &item.message))
                        .collect();
                    let mut results = Vec::new();
                    for call in &current.calls {
                        let message =
                            if let Some(message) = staged.get(call.result_entry_id.as_str()) {
                                (*message).clone()
                            } else {
                                match placed.get(&call.result_entry_id) {
                                    Some(Entry::Message { message, .. }) => {
                                        match staged_tool_result_message(message) {
                                            Some(message) => message,
                                            None => anyhow::bail!(
                                            "Completed tool call {} is missing its result entry",
                                            call.result_entry_id
                                        ),
                                        }
                                    }
                                    _ => anyhow::bail!(
                                        "Completed tool call {} is missing its result entry",
                                        call.result_entry_id
                                    ),
                                }
                            };
                        results.push(message);
                    }
                    turn_results = Some(results);
                }
                Ok(LaneCommand::Return {
                    result: Some(PlacementRead {
                        items,
                        turn_results,
                    }),
                })
            })
        },
        call_context,
    )
    .await
}

/// Upstream `commitPlacement` (`tool-placement.ts:137-246`): durably place
/// the ready results, advancing the batch or completing the operation into
/// its checkpoint.
async fn commit_placement(
    lane: std::sync::Arc<Lane>,
    drive: &Drive,
    // Upstream anchors the settled capability; the lane operation IS the
    // current state here, so the handle remains an identity anchor only.
    _capability: &OperationState,
    read: &PlacementRead,
) -> anyhow::Result<bool> {
    let context = drive.context().clone();
    let operation_id = drive.operation_id().to_string();
    let read = read.clone();
    let usage_ids: Vec<Option<String>> = read
        .items
        .iter()
        .map(|item| {
            item.message
                .usage
                .as_ref()
                .map(|_| lane.session().id_generator().next(None))
        })
        .collect();
    let closure_lane = lane.clone();
    let call_context = context.clone();
    closure_lane
        .settle_operation(
            move |state, run, _meta, reader| {
                let read = read.clone();
                let usage_ids = usage_ids.clone();
                let lane = lane.clone();
                let operation_id = operation_id.clone();
                let context = context.clone();
                Box::pin(async move {
                    let OperationPhase::Tools { batch: current } = &run.phase else {
                        anyhow::bail!("Tool placement requires a tools operation");
                    };
                    let mut writes: Vec<Write> = Vec::new();
                    struct EntryEvent {
                        entry: NewEntry,
                        seq_index: usize,
                        usage: Option<(UsageRow, usize)>,
                    }
                    let mut event_entries: Vec<EntryEvent> = Vec::new();
                    let mut parent_id = state.tip_id.clone();
                    for (index, item) in read.items.iter().enumerate() {
                        let terminate = match item.call.state {
                            ToolCallState::OutcomeReady { terminate } => Some(terminate),
                            _ => None,
                        };
                        let entry = NewEntry::Message {
                            id: item.call.result_entry_id.clone(),
                            parent_id: parent_id.clone(),
                            message: AgentMessage::ToolResult(item.message.clone()),
                            terminate,
                        };
                        let seq_index = writes.len();
                        writes.push(insert_entry(entry.clone()));
                        writes.push(delete_value(&pending_entry(&item.call.result_entry_id)));
                        let mut usage = None;
                        if let (Some(usage_id), Some(usage_value)) =
                            (&usage_ids[index], &item.message.usage)
                        {
                            let new_row = NewUsageRow {
                                id: usage_id.clone(),
                                usage: *usage_value,
                                entry_id: Some(item.call.result_entry_id.clone()),
                                adjustment: false,
                                details: None,
                            };
                            let seq_index_usage = writes.len();
                            writes.push(insert_usage(new_row.clone()));
                            usage = Some((
                                UsageRow {
                                    id: new_row.id,
                                    seq: 0,
                                    usage: new_row.usage,
                                    entry_id: new_row.entry_id,
                                    adjustment: new_row.adjustment,
                                    details: new_row.details,
                                },
                                seq_index_usage,
                            ));
                        }
                        event_entries.push(EntryEvent {
                            entry,
                            seq_index,
                            usage,
                        });
                        parent_id = Some(item.call.result_entry_id.clone());
                    }

                    let mut completed_calls = current.calls.clone();
                    for call in &mut completed_calls {
                        if let Some(item) = read.items.iter().find(|candidate| {
                            candidate.call.source_index == call.source_index
                                && candidate.call.result_entry_id == call.result_entry_id
                        }) {
                            let terminate = match item.call.state {
                                ToolCallState::OutcomeReady { terminate } => terminate,
                                _ => false,
                            };
                            call.state = ToolCallState::Completed { terminate };
                        }
                    }
                    let complete = completed_calls
                        .iter()
                        .all(|call| matches!(call.state, ToolCallState::Completed { .. }));
                    writes.push(set_value(
                        &branch_tip(lane.name()),
                        parent_id
                            .clone()
                            .map(serde_json::Value::String)
                            .unwrap_or(serde_json::Value::Null),
                    ));

                    let next_run = if complete {
                        let all_terminate = completed_calls.iter().all(|call| {
                            matches!(call.state, ToolCallState::Completed { terminate: true })
                        });
                        let args = reader
                            .scan_values(
                                &operation_tool_args_prefix(&operation_id, Some(&current.turn_id)),
                                context.clone(),
                            )
                            .await?;
                        for stored in &args {
                            writes.push(delete_value(&stored.address));
                        }
                        let trigger_entry_id = parent_id
                            .clone()
                            .ok_or_else(|| anyhow::anyhow!("Tool placement has no tip"))?;
                        OperationState {
                            scope: run.scope.clone(),
                            phase: OperationPhase::Checkpoint {
                                checkpoint: CheckpointData {
                                    continuation: if all_terminate {
                                        Continuation::MayFinish {
                                            include_final_assistant: false,
                                        }
                                    } else {
                                        Continuation::NeedAssistant {
                                            overflow_recovery_used: false,
                                        }
                                    },
                                    trigger_entry_id,
                                },
                            },
                        }
                    } else {
                        let mut batch = current.clone();
                        batch.calls = completed_calls;
                        with_tool_batch(run, batch)
                    };

                    let lane_name = lane.name().to_string();
                    Ok(OperationCommand::Commit {
                        writes,
                        operation_state: next_run,
                        lane: Some(LanePatch {
                            tip_id: parent_id,
                            configuration: Some(state.configuration.clone()),
                            inbox: None,
                        }),
                        materialize: Box::new(move |_commit| complete),
                        events: Some(Box::new(move |commit| {
                            let mut events = Vec::new();
                            for entry_event in &event_entries {
                                let Some(seq) = commit.seqs.get(entry_event.seq_index).copied()
                                else {
                                    anyhow::bail!("commit sequence missing for placed tool entry");
                                };
                                events.push(HarnessEvent::EntryAdded {
                                    lane: lane_name.clone(),
                                    entry: materialize_entry(
                                        &entry_event.entry,
                                        seq,
                                        commit.timestamp,
                                    )?,
                                });
                                if let Some((row, usage_seq_index)) = &entry_event.usage {
                                    let Some(usage_seq) =
                                        commit.seqs.get(*usage_seq_index).copied()
                                    else {
                                        anyhow::bail!("commit sequence missing for usage row");
                                    };
                                    let mut row = row.clone();
                                    row.seq = usage_seq;
                                    events.push(HarnessEvent::Usage {
                                        lane: lane_name.clone(),
                                        totals: commit.stats.usage,
                                        row,
                                    });
                                }
                            }
                            Ok(events)
                        })),
                    })
                })
            },
            call_context,
        )
        .await
}

fn materialize_entry(entry: &NewEntry, seq: i64, timestamp: i64) -> anyhow::Result<Entry> {
    Ok(match entry {
        NewEntry::Message {
            id,
            parent_id,
            message,
            terminate,
        } => Entry::Message {
            id: id.clone(),
            parent_id: parent_id.clone(),
            seq,
            timestamp,
            message: message.clone(),
            terminate: *terminate,
        },
        _ => anyhow::bail!("Tool placement only materializes message entries"),
    })
}

/// Upstream `materializeReady` (`tool-placement.ts:248-294`): emit the staged
/// results, commit their placement, and emit the turn end on completion.
pub async fn materialize_ready(
    lane: &std::sync::Arc<Lane>,
    drive: &Drive,
    capability: &OperationState,
    sources: &ToolBatchSource,
    recovery: bool,
) -> anyhow::Result<()> {
    let Some(read) = read_placement(lane, drive, sources).await? else {
        return Ok(());
    };
    let context = drive.context().clone();
    let mut events = Vec::new();
    for item in &read.items {
        events.push(HarnessEvent::MessageStart {
            recovery,
            lane: lane.name().to_string(),
            run_id: Some(drive.operation_id().to_string()),
            message: AgentMessage::ToolResult(item.message.clone()),
        });
        events.push(HarnessEvent::MessageEnd {
            recovery,
            lane: lane.name().to_string(),
            run_id: Some(drive.operation_id().to_string()),
            message: AgentMessage::ToolResult(item.message.clone()),
            entry_id: item.call.result_entry_id.clone(),
        });
    }
    lane.emit_batch(events, context.clone()).await?;
    let complete = commit_placement(lane.clone(), drive, capability, &read).await?;
    if complete {
        if let Some(turn_results) = &read.turn_results {
            let OperationPhase::Tools { batch } = &capability.phase else {
                anyhow::bail!("Tool placement requires a tools operation");
            };
            lane.emit_batch(
                vec![HarnessEvent::TurnEnd {
                    lane: lane.name().to_string(),
                    run_id: drive.operation_id().to_string(),
                    turn_id: batch.turn_id.clone(),
                    message: AgentMessage::Assistant(sources.assistant.clone()),
                    tool_results: turn_results.clone(),
                    recovery,
                }],
                context,
            )
            .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core::harness::runtime::lane::{EmitBatch, RuntimeConfig};
    use crate::agent_core::harness::runtime::restore::restore_lane;
    use crate::agent_core::harness::session::{
        self as session, MemoryStorage, SessionMetadata, StorageBackedSession,
    };
    use crate::agent_core::harness::session::{LaneConfiguration, LaneModel};
    use crate::agent_core::harness::{background_context, DEFAULT_COMPACTION_SETTINGS};
    use crate::agent_core::types::{QueueMode, ThinkingLevel, ToolExecutionMode};
    use crate::ai::models::faux::{faux_provider, FauxProviderOptions};
    use crate::ai::models::{create_models, CreateModelsOptions};

    fn assistant_entry_writes(id: &str, blocks: serde_json::Value) -> Vec<Write> {
        let message: AgentMessage = serde_json::from_value(serde_json::json!({
            "role": "assistant",
            "content": blocks,
            "api": "faux",
            "provider": "faux",
            "model": "faux-1",
            "stopReason": "toolUse",
            "usage": {"input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0,
                "totalTokens": 2, "cost": {"input": 0, "output": 0, "cacheRead": 0,
                "cacheWrite": 0, "total": 0}},
            "timestamp": 5,
        }))
        .unwrap();
        vec![insert_entry(NewEntry::Message {
            id: id.to_string(),
            parent_id: None,
            message,
            terminate: None,
        })]
    }

    async fn create_lane() -> (std::sync::Arc<Lane>, std::sync::Arc<StorageBackedSession>) {
        let storage = MemoryStorage::new(session::MemoryStorageOptions::default());
        let sess = std::sync::Arc::new(StorageBackedSession::new(
            SessionMetadata {
                id: "tool-placement-test".into(),
                created_at: 1,
                storage_version: 1,
                ..Default::default()
            },
            std::sync::Arc::new(storage),
        ));
        let configuration = LaneConfiguration {
            model: LaneModel {
                provider: "faux".to_string(),
                model_id: "faux-1".to_string(),
            },
            thinking_level: ThinkingLevel::Off,
            active_tool_names: Vec::new(),
        };
        let mut writes = vec![
            session::set_value(&session::branch_tip("main"), serde_json::Value::Null),
            session::set_value(
                &session::lane_config("main"),
                session::lane_configuration_value(&configuration),
            ),
            session::set_value(
                &session::lane_state("main"),
                serde_json::json!({"currentOperationId":null,"lastOperationId":null,"inbox":[]}),
            ),
        ];
        writes.extend(assistant_entry_writes(
            "a1",
            serde_json::json!([
                {"type": "text", "text": "hello"},
                {"type": "toolCall", "id": "call_1", "name": "bash",
                 "arguments": {"cmd": "ls"}},
            ]),
        ));
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
        let emit: EmitBatch = std::sync::Arc::new(|events, _context| {
            Box::pin(async move {
                let _ = events;
                Ok(())
            })
        });
        let lane = Lane::new(
            "main",
            sess.clone(),
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
        );
        (lane, sess)
    }

    #[tokio::test]
    async fn tool_call_for_resolves_and_rejects_source_indexes() {
        let sources = ToolBatchSource {
            assistant: serde_json::from_str(
                r#"{"role":"assistant","content":[],"api":"faux","provider":"faux","model":"faux-1","stopReason":"stop","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"timestamp":5}"#,
            )
            .unwrap(),
            calls: serde_json::from_str::<HashMap<usize, AgentToolCall>>(
                r#"{"2": {"id": "call_1", "name": "bash", "arguments": {}}}"#,
            )
            .unwrap(),
        };
        let call = ToolCall {
            source_index: 2,
            result_entry_id: "r1".to_string(),
            state: ToolCallState::Planned,
        };
        assert_eq!(tool_call_for(&sources, &call).unwrap().id, "call_1");
        let missing = ToolCall {
            source_index: 3,
            result_entry_id: "r2".to_string(),
            state: ToolCallState::Planned,
        };
        let error = tool_call_for(&sources, &missing).unwrap_err();
        assert!(error.to_string().contains("source index 3 is invalid"));
    }

    #[tokio::test]
    async fn with_tool_batch_preserves_scope_and_swaps_phase_payload() {
        let run = OperationState {
            scope: serde_json::from_str(
                r#"{"control":{"status":"running"},"settings":{"compaction":{"enabled":true,"reserveTokens":16384,"keepRecentTokens":20000},"steeringMode":"all","followUpMode":"all","toolExecution":"parallel"},"latestAssistantEntryId":null}"#,
            )
            .unwrap(),
            phase: OperationPhase::Starting,
        };
        let batch = ToolBatch {
            assistant_entry_id: "a1".to_string(),
            configuration: LaneConfiguration {
                model: LaneModel {
                    provider: "p".to_string(),
                    model_id: "m".to_string(),
                },
                thinking_level: ThinkingLevel::Off,
                active_tool_names: vec![],
            },
            turn_id: "t1".to_string(),
            calls: vec![],
        };
        let next = with_tool_batch(&run, batch);
        assert_eq!(next.scope, run.scope);
        assert!(matches!(next.phase, OperationPhase::Tools { .. }));
    }

    #[tokio::test]
    async fn read_tool_batch_source_validates_entry_and_indexes() {
        let (lane, _sess) = create_lane().await;
        let drive = Drive::new(
            &crate::agent_core::harness::runtime::drive_pass::DriveOptions {
                operation_id: "op1".to_string(),
                wait_for_retry: None,
                poll_deferred: None,
            },
            background_context(),
        );
        let batch = ToolBatch {
            assistant_entry_id: "a1".to_string(),
            configuration: LaneConfiguration {
                model: LaneModel {
                    provider: "faux".to_string(),
                    model_id: "faux-1".to_string(),
                },
                thinking_level: ThinkingLevel::Off,
                active_tool_names: vec![],
            },
            turn_id: "t1".to_string(),
            calls: vec![ToolCall {
                source_index: 1,
                result_entry_id: "r1".to_string(),
                state: ToolCallState::Planned,
            }],
        };
        let sources = read_tool_batch_source(&lane, &drive, &batch).await.unwrap();
        assert_eq!(sources.calls.get(&1).unwrap().id, "call_1");
        let mut bad_batch = batch.clone();
        bad_batch.calls[0].source_index = 0;
        let error = read_tool_batch_source(&lane, &drive, &bad_batch)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("does not name a tool-call block"),
            "unexpected error: {error}"
        );
    }
}
