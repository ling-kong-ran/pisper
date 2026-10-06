//! Mechanical terminal transaction suffix and result records from
//! runtime/drive/terminal.ts. These helpers do not commit or publish anything.
use super::super::durable::{OperationMeta, OperationPhase, OperationState, ToolCallState};
use crate::agent_core::harness::context::Context;
use crate::agent_core::harness::session::{
    delete_list, delete_value, operation_meta, operation_preparation_prefix, operation_state,
    operation_tool_args_prefix, operation_tool_memo_prefix, pending_assistant_frames,
    pending_entry, pending_tool_output_prefix, OperationError, OperationResultRecord,
    SessionInvariantError, SessionMutationReader, TerminalStatus, Write,
};
use std::collections::HashSet;

pub async fn operation_cleanup_writes(
    reader: &dyn SessionMutationReader,
    operation_id: &str,
    state: &OperationState,
    context: Context,
) -> anyhow::Result<Vec<Write>> {
    let args = operation_tool_args_prefix(operation_id, None);
    let memos = operation_tool_memo_prefix(operation_id, None);
    let preparation = operation_preparation_prefix(operation_id);
    let output = pending_tool_output_prefix(operation_id);
    let (args, memos, preparations, outputs) = futures::try_join!(
        reader.scan_values(&args, context.clone()),
        reader.scan_values(&memos, context.clone()),
        reader.scan_values(&preparation, context.clone()),
        reader.scan_values(&output, context),
    )?;
    let mut writes = vec![
        delete_value(&operation_meta(operation_id)),
        delete_value(&operation_state(operation_id)),
    ];
    writes.extend(
        args.into_iter()
            .chain(memos)
            .chain(preparations)
            .chain(outputs)
            .map(|value| delete_value(&value.address)),
    );
    match &state.phase {
        OperationPhase::AssistantEffectPending {
            response_entry_id, ..
        }
        | OperationPhase::DeferredEffectPending {
            response_entry_id, ..
        } => {
            writes.push(delete_list(&pending_assistant_frames(
                operation_id,
                response_entry_id,
            )));
        }
        OperationPhase::Tools { batch } => {
            let mut seen = HashSet::new();
            for call in &batch.calls {
                if matches!(call.state, ToolCallState::OutcomeReady { .. })
                    && seen.insert(&call.result_entry_id)
                {
                    writes.push(delete_value(&pending_entry(&call.result_entry_id)));
                }
            }
        }
        _ => {}
    }
    Ok(writes)
}

pub fn operation_result_record(
    meta: &OperationMeta,
    status: TerminalStatus,
    tip_id: Option<String>,
    error: Option<OperationError>,
) -> anyhow::Result<OperationResultRecord> {
    operation_result_record_at(meta, status, tip_id, error, crate::ai::now_ms())
}

/// Explicit clock input is a testable equivalent of upstream Date.now().
pub fn operation_result_record_at(
    meta: &OperationMeta,
    status: TerminalStatus,
    tip_id: Option<String>,
    error: Option<OperationError>,
    ended_at: i64,
) -> anyhow::Result<OperationResultRecord> {
    if (status == TerminalStatus::Failed) != error.is_some() {
        return Err(SessionInvariantError(
            "Only a failed operation result may carry an error".into(),
        )
        .into());
    }
    Ok(OperationResultRecord {
        operation_id: meta.operation_id.clone(),
        kind: meta.intent.kind().into(),
        status,
        error,
        from_tip_id: meta.source_tip_id.clone(),
        tip_id,
        started_at: meta.started_at,
        ended_at,
    })
}
