//! Pure event folding, port of upstream runtime/reducer.ts.
use super::projection::*;
use crate::agent_core::harness::session::{Entry, OperationResultRecord, TerminalStatus};
use crate::agent_core::types::AgentMessage;
use crate::ai::types::StopReason;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneSnapshotReduction {
    Stable,
    Rebase,
}
fn matching<'a>(snapshot: &'a mut LaneSnapshot, id: &str) -> Option<&'a mut LaneOperationSnapshot> {
    snapshot
        .operation
        .as_mut()
        .filter(|operation| operation.id == id)
}
fn start(snapshot: &mut LaneSnapshot, id: &str, kind: OperationKind, started_at: i64) {
    snapshot.operation = Some(LaneOperationSnapshot {
        id: id.into(),
        kind,
        started_at,
        from_tip_id: snapshot.tip_id.clone(),
        status: OperationStatus::Open,
        retry: None,
        deferred: None,
        streaming_message: None,
        running_tools: Vec::new(),
    });
}

pub fn reduce_lane_snapshot(
    snapshot: &mut LaneSnapshot,
    event: &SnapshotEvent,
) -> LaneSnapshotReduction {
    use SnapshotEventPayload::*;
    if event
        .lane
        .as_ref()
        .is_some_and(|lane| lane != &snapshot.lane)
        && !matches!(event.payload, Usage { .. })
    {
        return LaneSnapshotReduction::Stable;
    }
    match &event.payload {
        RunStart { run_id, started_at } => start(snapshot, run_id, OperationKind::Run, *started_at),
        CompactionStart { run_id, started_at } => {
            if snapshot.operation.is_none() {
                start(snapshot, run_id, OperationKind::Compaction, *started_at);
            }
        }
        NavigationStart { run_id, started_at } => {
            start(snapshot, run_id, OperationKind::Navigation, *started_at)
        }
        OperationAbort { operation_id } => {
            if let Some(op) = matching(snapshot, operation_id) {
                op.status = OperationStatus::Aborting;
            }
        }
        RunResume { run_id } => {
            if let Some(op) = matching(snapshot, run_id) {
                op.deferred = None;
            }
        }
        RunSuspend {
            run_id,
            deferred,
            poll,
        } => {
            if let Some(op) = matching(snapshot, run_id) {
                op.streaming_message = None;
                op.deferred = Some(DeferredSnapshot {
                    handle: deferred.clone(),
                    poll: *poll,
                });
            }
        }
        RetryScheduled {
            run_id,
            attempt,
            max_attempts,
            not_before,
        } => {
            if let Some(op) = matching(snapshot, run_id) {
                op.retry = Some(RetrySnapshot {
                    attempt: *attempt,
                    max_attempts: *max_attempts,
                    next_attempt_at: *not_before,
                });
            }
        }
        RetryStart { run_id } | RetryEnd { run_id } => {
            if let Some(op) = matching(snapshot, run_id) {
                op.retry = None;
            }
        }
        MessageStart {
            run_id: Some(run_id),
            message,
        } => {
            if matches!(message,AgentMessage::Assistant(m) if m.stop_reason==StopReason::Pending) {
                if let Some(op) = matching(snapshot, run_id) {
                    op.streaming_message = Some(message.clone());
                }
            }
        }
        MessageUpdate { run_id, message } => {
            if matches!(message, AgentMessage::Assistant(_)) {
                if let Some(op) = matching(snapshot, run_id) {
                    op.streaming_message = Some(message.clone());
                }
            }
        }
        MessageEnd {
            run_id: Some(run_id),
        } => {
            if let Some(op) = matching(snapshot, run_id) {
                op.streaming_message = None;
            }
        }
        ToolStart {
            run_id,
            tool_call_id,
            tool_name,
            args,
        } => {
            if let Some(op) = matching(snapshot, run_id) {
                let tool = LaneSnapshotTool {
                    tool_call_id: tool_call_id.clone(),
                    tool_name: tool_name.clone(),
                    args: args.clone(),
                    state: SnapshotToolState::Running { result: None },
                };
                if let Some(index) = op
                    .running_tools
                    .iter()
                    .position(|t| t.tool_call_id == *tool_call_id)
                {
                    op.running_tools[index] = tool;
                } else {
                    op.running_tools.push(tool);
                }
            }
        }
        ToolUpdate {
            run_id,
            tool_call_id,
            partial_result,
        } => {
            if let Some(op) = matching(snapshot, run_id) {
                if let Some(LaneSnapshotTool {
                    state: SnapshotToolState::Running { result },
                    ..
                }) = op
                    .running_tools
                    .iter_mut()
                    .find(|t| t.tool_call_id == *tool_call_id)
                {
                    *result = Some(partial_result.clone());
                }
            }
        }
        ToolEnd {
            run_id,
            tool_call_id,
            tool_name,
            result,
            is_error,
        } => {
            if let Some(op) = matching(snapshot, run_id) {
                if let Some(tool) = op
                    .running_tools
                    .iter_mut()
                    .find(|t| t.tool_call_id == *tool_call_id)
                {
                    tool.tool_name = tool_name.clone();
                    tool.state = SnapshotToolState::Settled {
                        result: result.clone(),
                        is_error: *is_error,
                    };
                }
            }
        }
        EntryAdded { entry } => {
            if let Entry::Message {
                message: AgentMessage::ToolResult(message),
                ..
            } = entry
            {
                if let Some(op) = snapshot.operation.as_mut() {
                    if let Some(index) = op
                        .running_tools
                        .iter()
                        .position(|t| t.tool_call_id == message.tool_call_id)
                    {
                        op.running_tools.remove(index);
                    }
                }
            }
            if matches!(entry, Entry::Compaction { .. }) {
                snapshot.transcript.clear();
            }
            snapshot.transcript.push(entry.clone());
            snapshot.tip_id = Some(entry.id().into());
            if matches!(entry, Entry::Message { .. }) {
                snapshot.stats.message_count += 1;
            }
        }
        QueueUpdate { queues } => snapshot.queues = queues.clone(),
        Usage { totals } => snapshot.stats.usage = *totals,
        ConfigUpdate { change } => {
            if event.lane.as_ref() == Some(&snapshot.lane) {
                match change {
                    SnapshotConfigChange::Model { value } => {
                        snapshot.configuration.model = value.clone()
                    }
                    SnapshotConfigChange::ThinkingLevel { value } => {
                        snapshot.configuration.thinking_level = *value
                    }
                    SnapshotConfigChange::ActiveTools { value } => {
                        snapshot.configuration.active_tool_names = value.clone()
                    }
                    SnapshotConfigChange::Other => {}
                }
            }
        }
        RunEnd {
            run_id,
            status,
            error,
            from_tip_id,
            tip_id,
            ended_at,
        } => {
            if let Some(op) = matching(snapshot, run_id).filter(|op| op.kind == OperationKind::Run)
            {
                let record = OperationResultRecord {
                    operation_id: run_id.clone(),
                    kind: "run".into(),
                    status: *status,
                    error: if *status == TerminalStatus::Failed {
                        error.clone()
                    } else {
                        None
                    },
                    from_tip_id: from_tip_id.clone(),
                    tip_id: tip_id.clone(),
                    started_at: op.started_at,
                    ended_at: *ended_at,
                };
                snapshot.last_result = Some(record);
                snapshot.operation = None;
                snapshot.tip_id = tip_id.clone();
            }
        }
        CompactionEnd {
            run_id,
            status,
            error,
            ended_at,
        } => {
            if let Some(op) = snapshot
                .operation
                .as_ref()
                .filter(|op| op.id == *run_id && op.kind == OperationKind::Compaction)
            {
                snapshot.last_result = Some(OperationResultRecord {
                    operation_id: run_id.clone(),
                    kind: "compaction".into(),
                    status: *status,
                    error: if *status == TerminalStatus::Failed {
                        error.clone()
                    } else {
                        None
                    },
                    from_tip_id: op.from_tip_id.clone(),
                    tip_id: snapshot.tip_id.clone(),
                    started_at: op.started_at,
                    ended_at: *ended_at,
                });
                snapshot.operation = None;
            }
        }
        NavigationEnd => return LaneSnapshotReduction::Rebase,
        Fault => snapshot.faulted = true,
        MessageStart { run_id: None, .. }
        | MessageEnd { run_id: None }
        | HandlerError
        | TurnStart
        | TurnEnd
        | ValueUpdate
        | LaneCreated => {}
    }
    LaneSnapshotReduction::Stable
}
