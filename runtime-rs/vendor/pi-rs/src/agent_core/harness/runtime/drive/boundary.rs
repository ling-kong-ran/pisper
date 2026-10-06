//! Port of `packages/agent/src/harness/runtime/drive/boundary.ts` (262
//! lines): the boundary vocabulary shared by run completion — retry-policy
//! normalization, assistant-ready generation state construction, lane-inbox
//! planning, placement event materialization, and the run-finishing boundary
//! replan.
//!
//! Disclosed scope: `finish_run_boundary`'s before_run_end hook chain and
//! follow-up injection are fully implemented; their end-to-end oracle lands
//! with the generation slice that drives them.

use std::collections::HashSet;
use std::sync::Arc;

use crate::agent_core::harness::context::Context;
use crate::agent_core::harness::runtime::drive_pass::{Drive, ProcedureResult};
use crate::agent_core::harness::runtime::durable::{
    GenerationContext, NormalizedRetryPolicy, OperationPhase, OperationScope, OperationState,
};
use crate::agent_core::harness::runtime::events::{HarnessEvent, RunEndStatus};
use crate::agent_core::harness::runtime::lane::{
    ContinueOperationResult, Lane, LanePatch, OperationCommand,
};
use crate::agent_core::harness::runtime::projection::LaneQueuedItem;
use crate::agent_core::harness::runtime::transcript::{
    committed_entry_events, entry_lifecycle_events, read_bounded_context, read_lane_queues,
};
use crate::agent_core::harness::session::commit::insert_entry;
use crate::agent_core::harness::session::types::{
    CommitResult, Entry, InboxItem, InboxItemKind, NewEntry, PendingEntry, TerminalStatus, Write,
};
use crate::agent_core::harness::session::values::{
    branch_tip, delete_value, pending_entry, set_value,
};
use crate::agent_core::harness::session::Session as _;
use crate::agent_core::types::{AgentMessage, QueueMode};
use crate::ai::types::message::{StringOrBlocks, UserMessage};

/// Upstream `BoundaryFinishPending`.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // parity export; consumed by the later generation slice
pub struct BoundaryFinishPending {
    pub entry_ids: Vec<String>,
}

#[cfg(test)]
mod boundary_tests;

/// Upstream `BoundaryPlacement`.
#[derive(Debug, Clone)]
pub struct BoundaryPlacement {
    pub entries: Vec<NewEntry>,
    pub writes: Vec<Write>,
    pub tip_id: Option<String>,
    pub inbox: Vec<InboxItem>,
    pub trigger_entry_id: Option<String>,
    pub queues: Option<Vec<LaneQueuedItem>>,
}

fn inbox_kind(kind: InboxItemKind) -> &'static str {
    match kind {
        InboxItemKind::Steer => "steer",
        InboxItemKind::FollowUp => "followUp",
        InboxItemKind::NextRun => "nextRun",
        InboxItemKind::Write => "write",
    }
}

/// Upstream `normalizedRetryPolicy` (`boundary.ts:44-53`): project the lane
/// configuration's retry policy onto the durable generation vocabulary.
pub fn normalized_retry_policy(lane: &Lane) -> NormalizedRetryPolicy {
    let retry = lane.read_config().retry_policy;
    NormalizedRetryPolicy {
        max_attempts: if retry.enabled {
            retry.max_retries + 1
        } else {
            1
        },
        base_delay_ms: retry.base_delay_ms,
        max_agent_delay_ms: retry
            .max_agent_delay_ms
            .unwrap_or(crate::ai::retry::DEFAULT_MAX_AGENT_RETRY_DELAY_MS),
    }
}

/// Upstream `assistantReadyAtBoundary` (`boundary.ts:55-76`): the next
/// assistant generation state at a boundary, minting its step id.
pub fn assistant_ready_at_boundary(
    lane: &Lane,
    state: &crate::agent_core::harness::runtime::durable::LaneState,
    scope: &OperationScope,
    trigger_entry_id: String,
    overflow_recovery_used: bool,
) -> OperationState {
    let config = lane.read_config();
    OperationState {
        scope: scope.clone(),
        phase: OperationPhase::AssistantReady {
            generation_context: GenerationContext {
                step_id: lane.session().id_generator().next(None),
                trigger_entry_id,
                configuration: state.configuration.clone(),
                stream_options: config.stream_options.clone(),
                retry_policy: normalized_retry_policy(lane),
                overflow_recovery_used,
            },
            next_attempt: 1,
        },
    }
}

/// Upstream `planBoundaryInbox` (`boundary.ts:79-148`): select and
/// materialize one boundary's lane-owned input without committing it.
pub async fn plan_boundary_inbox(
    lane: &Lane,
    state: &crate::agent_core::harness::runtime::durable::LaneState,
    scope: &OperationScope,
    reader: &dyn crate::agent_core::harness::session::SessionMutationReader,
    tip_id: Option<String>,
    follow_up_when_no_trigger: bool,
    context: &crate::agent_core::harness::context::Context,
) -> anyhow::Result<BoundaryPlacement> {
    let projectors = |custom_type: &str| -> bool {
        lane.read_config()
            .entry_projectors
            .as_ref()
            .map(|map| map.contains_key(custom_type))
            .unwrap_or(false)
    };
    let projects = |pending: &PendingEntry| -> bool {
        match pending {
            PendingEntry::Message { .. } => true,
            PendingEntry::Custom { custom_type, .. } => projectors(custom_type),
        }
    };
    async fn load_pending(
        reader: &dyn crate::agent_core::harness::session::SessionMutationReader,
        items: Vec<InboxItem>,
        context: Context,
    ) -> anyhow::Result<Vec<(InboxItem, PendingEntry)>> {
        let mut out = Vec::new();
        for item in &items {
            let stored = reader
                .get_value(&pending_entry(&item.entry_id), context.clone())
                .await?
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Pending {} entry {} is missing its payload",
                        inbox_kind(item.kind),
                        item.entry_id
                    )
                })?;
            let pending: PendingEntry = serde_json::from_value(stored.value)
                .map_err(|error| anyhow::anyhow!("invalid pending payload: {error}"))?;
            if !matches!(pending, PendingEntry::Message { .. }) && item.kind != InboxItemKind::Write
            {
                return Err(anyhow::anyhow!(
                    "Queued {} entry {} is not a message",
                    inbox_kind(item.kind),
                    item.entry_id
                ));
            }
            out.push((item.clone(), pending));
        }
        Ok(out)
    }

    let is_steer = |item: &InboxItem| matches!(item.kind, InboxItemKind::Steer);
    let steer_count = state.inbox.iter().filter(|item| is_steer(item)).count();
    let steer_take = if scope.settings.steering_mode == QueueMode::All {
        steer_count
    } else {
        steer_count.min(1)
    };
    let mut selected_indices: Vec<usize> = state
        .inbox
        .iter()
        .enumerate()
        .filter(|(_, item)| matches!(item.kind, InboxItemKind::Write) || is_steer(item))
        .map(|(index, _)| index)
        .collect();
    // Apply the steer cap in inbox order: keep every write, plus at most
    // `steer_take` steers.
    if scope.settings.steering_mode != QueueMode::All {
        let mut steers_seen = 0usize;
        selected_indices.retain(|index| {
            let item = &state.inbox[*index];
            if is_steer(item) {
                steers_seen += 1;
                steers_seen <= 1
            } else {
                true
            }
        });
    }
    let _ = steer_take;
    let mut pending = load_pending(
        reader,
        selected_indices
            .iter()
            .map(|index| state.inbox[*index].clone())
            .collect(),
        context.clone(),
    )
    .await?;
    if follow_up_when_no_trigger && !pending.iter().any(|(_, value)| projects(value)) {
        let follow_up_count = state
            .inbox
            .iter()
            .filter(|item| matches!(item.kind, InboxItemKind::FollowUp))
            .count();
        let follow_up_take = if scope.settings.follow_up_mode == QueueMode::All {
            follow_up_count
        } else {
            follow_up_count.min(1)
        };
        let mut follow_up_seen = 0usize;
        let mut extra = Vec::new();
        for (index, item) in state.inbox.iter().enumerate() {
            if !matches!(item.kind, InboxItemKind::FollowUp) {
                continue;
            }
            follow_up_seen += 1;
            if follow_up_seen <= follow_up_take {
                extra.push(index);
            }
        }
        for index in extra {
            if !selected_indices.contains(&index) {
                selected_indices.push(index);
            }
        }
        selected_indices.sort_unstable();
        pending = load_pending(
            reader,
            selected_indices
                .iter()
                .map(|index| state.inbox[*index].clone())
                .collect(),
            context.clone(),
        )
        .await?;
    }

    let mut parent_id = tip_id;
    let mut trigger_entry_id = None;
    let mut entries = Vec::new();
    for (item, value) in &pending {
        let entry = match value {
            PendingEntry::Message { payload } => NewEntry::Message {
                id: item.entry_id.clone(),
                parent_id: parent_id.clone(),
                message: payload.clone(),
                terminate: None,
            },
            PendingEntry::Custom {
                custom_type,
                payload,
            } => NewEntry::Custom {
                id: item.entry_id.clone(),
                parent_id: parent_id.clone(),
                custom_type: custom_type.clone(),
                data: payload.clone(),
            },
        };
        parent_id = Some(item.entry_id.clone());
        if projects(value) {
            trigger_entry_id = Some(item.entry_id.clone());
        }
        entries.push(entry);
    }
    let selected_ids: HashSet<&str> = selected_indices
        .iter()
        .map(|index| state.inbox[*index].entry_id.as_str())
        .collect();
    let inbox: Vec<InboxItem> = state
        .inbox
        .iter()
        .filter(|item| !selected_ids.contains(item.entry_id.as_str()))
        .map(|item| InboxItem {
            entry_id: item.entry_id.clone(),
            kind: item.kind,
        })
        .collect();
    let queues = if selected_indices.is_empty() {
        None
    } else {
        Some(read_lane_queues(reader, &inbox, context.clone()).await?)
    };
    let writes = [
        entries
            .iter()
            .cloned()
            .map(insert_entry)
            .collect::<Vec<_>>(),
        selected_indices
            .iter()
            .map(|index| delete_value(&pending_entry(&state.inbox[*index].entry_id)))
            .collect::<Vec<_>>(),
        if entries.is_empty() {
            Vec::new()
        } else {
            vec![set_value(
                &branch_tip(lane.name()),
                parent_id
                    .clone()
                    .map(serde_json::Value::String)
                    .unwrap_or(serde_json::Value::Null),
            )]
        },
    ]
    .concat();
    Ok(BoundaryPlacement {
        entries,
        writes,
        tip_id: parent_id,
        inbox,
        trigger_entry_id,
        queues,
    })
}

/// Upstream `boundaryPlacementEvents` (`boundary.ts:150-161`).
pub fn boundary_placement_events(
    placement: &BoundaryPlacement,
    commit: &CommitResult,
    first_write_index: usize,
    lane: &str,
    run_id: &str,
) -> anyhow::Result<Vec<HarnessEvent>> {
    let mut events: Vec<HarnessEvent> = committed_entry_events(
        &placement.entries,
        commit,
        lane,
        Some(run_id),
        first_write_index,
    )?
    .into_iter()
    .map(HarnessEvent::from)
    .collect();
    if let Some(queues) = &placement.queues {
        events.push(HarnessEvent::QueueUpdate {
            lane: lane.to_string(),
            queues: queues.clone(),
        });
    }
    Ok(events)
}

/// Upstream `finishRunBoundary` (`boundary.ts:164-262`): replan after
/// before_run_end and commit either renewed work or the terminal run result.
pub async fn finish_run_boundary(
    lane: &std::sync::Arc<Lane>,
    drive: &Drive,
    capability: &OperationState,
    continuation: &crate::agent_core::harness::runtime::durable::Continuation,
    planned_entry_ids: &[String],
    pending_events: Vec<HarnessEvent>,
    gate: Arc<dyn crate::agent_core::harness::hooks::Gate>,
) -> anyhow::Result<ProcedureResult> {
    let context = match read_bounded_context(lane, drive, capability).await? {
        ContinueOperationResult::CancelRequested => return Ok(ProcedureResult::Continue),
        ContinueOperationResult::Result { value } => value,
    };
    let hook = lane.hooks().run_with_gate(
        crate::agent_core::harness::hooks::HookInvocation {
            lane: lane.name().to_string(),
            run_id: drive.operation_id().to_string(),
            event: crate::agent_core::harness::hooks::HookEvent::BeforeRunEnd(
                crate::agent_core::harness::hooks::BeforeRunEndEvent { messages: context },
            ),
        },
        gate,
        drive.context().clone(),
    );
    let hook = hook.await?;
    let follow_up = match &hook {
        crate::agent_core::harness::hooks::HookResult::BeforeRunEnd(Some(result)) => {
            result.follow_up.clone()
        }
        _ => None,
    };
    let follow_up_entry = follow_up.map(|text| {
        (
            lane.session().id_generator().next(None),
            AgentMessage::User(UserMessage {
                content: StringOrBlocks::Text(text),
                timestamp: crate::ai::now_ms(),
            }),
        )
    });

    let lane_for_plan = lane.clone();
    let plan_context = drive.context().clone();
    let operation_id = drive.operation_id().to_string();
    let capability = capability.clone();
    let continuation = continuation.clone();
    let planned_entry_ids = planned_entry_ids.to_vec();
    let result = lane
        .continue_operation(
            move |state, current, meta, reader| {
                let lane = lane_for_plan.clone();
                let context = plan_context.clone();
                let operation_id = operation_id.clone();
                let source_tip_id = meta.source_tip_id.clone();
                let continuation = continuation.clone();
                let planned_entry_ids = planned_entry_ids.to_vec();
                let pending_events = pending_events.clone();
                let follow_up_entry = follow_up_entry.clone();
                let _ = &capability;
                let continuation = continuation.clone();
                Box::pin(async move {
                    let placement = plan_boundary_inbox(
                        &lane,
                        state,
                        &current.scope,
                        reader,
                        state.tip_id.clone(),
                        true,
                        &context,
                    )
                    .await?;
                    if let Some(trigger_entry_id) = placement.trigger_entry_id.clone() {
                        let placement = placement.clone();
                        let operation_state =
                            assistant_ready_at_boundary(&lane, state, &current.scope, trigger_entry_id, false);
                        return Ok(OperationCommand::Commit {
                            writes: placement.writes.clone(),
                            operation_state,
                            lane: Some(LanePatch {
                                tip_id: placement.tip_id.clone(),
                                configuration: None,
                                inbox: Some(placement.inbox.clone()),
                            }),
                            materialize: Box::new(|_commit| ProcedureResult::Continue),
                            events: Some(Box::new(move |commit: &CommitResult| {
                                Ok(boundary_placement_events(
                                    &placement,
                                    commit,
                                    0,
                                    lane.name(),
                                    &operation_id,
                                )?
                                .into_iter()
                                .chain(pending_events)
                                .collect())
                            })),
                        });
                    }
                    let hook_plan_is_current = placement.entries.len() == planned_entry_ids.len()
                        && placement
                            .entries
                            .iter()
                            .zip(planned_entry_ids.iter())
                            .all(|(entry, planned)| new_entry_id(entry) == planned);
                    if hook_plan_is_current {
                        if let Some((follow_up_id, follow_up_message)) = &follow_up_entry {
                            let entry = NewEntry::Message {
                                id: follow_up_id.clone(),
                                parent_id: placement.tip_id.clone(),
                                message: follow_up_message.clone(),
                                terminate: None,
                            };
                            let entry_write_index = placement.writes.len();
                            let operation_state = assistant_ready_at_boundary(
                                &lane,
                                state,
                                &current.scope,
                                follow_up_id.clone(),
                                false,
                            );
                            let mut writes = placement.writes.clone();
                            writes.push(insert_entry(entry.clone()));
                            writes.push(set_value(
                                &branch_tip(lane.name()),
                                serde_json::Value::String(follow_up_id.clone()),
                            ));
                            let run_id = operation_id.clone();
                            return Ok(OperationCommand::Commit {
                                writes,
                                operation_state,
                                lane: Some(LanePatch {
                                    tip_id: Some(follow_up_id.clone()),
                                    configuration: None,
                                    inbox: Some(placement.inbox.clone()),
                                }),
                                materialize: Box::new(|_commit| ProcedureResult::Continue),
                                events: Some(Box::new(move |commit: &CommitResult| {
                                    let mut events = boundary_placement_events(
                                        &placement,
                                        commit,
                                        0,
                                        lane.name(),
                                        &run_id,
                                    )?;
                                    let Some(seq) = commit.seqs.get(entry_write_index).copied()
                                    else {
                                        anyhow::bail!("commit sequence missing for follow-up");
                                    };
                                    let entry = materialize_new_entry(&entry, seq, commit.timestamp)?;
                                    events.extend(
                                        entry_lifecycle_events(&entry, lane.name(), Some(&run_id))
                                            .into_iter()
                                            .map(HarnessEvent::from),
                                    );
                                    Ok(events
                                        .into_iter()
                                        .chain(pending_events)
                                        .collect())
                                })),
                            });
                        }
                    }
                    let Some(tip_id) = placement.tip_id.clone() else {
                        anyhow::bail!("Completed run has no tip");
                    };
                    let crate::agent_core::harness::runtime::durable::Continuation::MayFinish {
                        include_final_assistant,
                    } = continuation
                    else {
                        anyhow::bail!("finish_run_boundary requires a may_finish continuation");
                    };
                    if include_final_assistant && current.scope.latest_assistant_entry_id.is_none() {
                        anyhow::bail!("Completed run is missing its final assistant");
                    }
                    let record = crate::agent_core::harness::runtime::drive::terminal::operation_result_record(
                        meta,
                        TerminalStatus::Completed,
                        placement.tip_id.clone(),
                        None,
                    )?;
                    let cleanup = crate::agent_core::harness::runtime::drive::terminal::operation_cleanup_writes(
                        reader,
                        &operation_id,
                        current,
                        context.clone(),
                    )
                    .await?;
                    let mut writes = placement.writes.clone();
                    writes.extend(cleanup);
                    let ended_at = record.ended_at;
                    Ok(OperationCommand::Finish {
                        writes,
                        record: record.clone(),
                        lane: Some(LanePatch {
                            tip_id: placement.tip_id.clone(),
                            configuration: None,
                            inbox: Some(placement.inbox.clone()),
                        }),
                        materialize: Box::new(move |_commit| ProcedureResult::Settled { outcome: record }),
                        events: Some(Box::new(move |commit: &CommitResult| {
                            let mut events = boundary_placement_events(
                                &placement,
                                commit,
                                0,
                                lane.name(),
                                &operation_id.clone(),
                            )?;
                            events.push(HarnessEvent::RunEnd {
                                lane: lane.name().to_string(),
                                run_id: operation_id.clone(),
                                status: RunEndStatus::Completed,
                                error: None,
                                from_tip_id: source_tip_id.clone(),
                                tip_id: Some(tip_id),
                                ended_at,
                            });
                            Ok(events
                                .into_iter()
                                .chain(pending_events)
                                .collect())
                        })),
                    })
                })
            },
            drive.context().clone(),
        )
        .await?;
    Ok(match result {
        ContinueOperationResult::CancelRequested => ProcedureResult::Continue,
        ContinueOperationResult::Result { value } => value,
    })
}

fn materialize_new_entry(entry: &NewEntry, seq: i64, timestamp: i64) -> anyhow::Result<Entry> {
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
        other => {
            let _ = other;
            anyhow::bail!("boundary follow-up only materializes message entries")
        }
    })
}

/// Upstream `entry.id` over the tagged `NewEntry` union (read-side helper).
fn new_entry_id(entry: &NewEntry) -> &str {
    match entry {
        NewEntry::Message { id, .. }
        | NewEntry::Compaction { id, .. }
        | NewEntry::Custom { id, .. } => id,
        _ => {
            let value = serde_json::to_value(entry).expect("serializable entry");
            match value.get("id").and_then(|id| id.as_str()) {
                Some(id) => Box::leak(id.to_string().into_boxed_str()),
                None => "",
            }
        }
    }
}
