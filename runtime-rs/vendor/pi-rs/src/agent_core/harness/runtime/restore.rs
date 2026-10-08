//! Coherent, read-only lane restoration: upstream runtime/restore.ts.
use super::durable::{
    LaneState, Operation, OperationIntent, OperationMeta, OperationPhase, OperationState,
    ResultBoundary,
};
use crate::agent_core::harness::context::Context;
use crate::agent_core::harness::session::{
    branch_tip, branch_tip_inventory_prefix, lane_config, lane_state, operation_meta,
    operation_state, LaneState as DurableLaneState, Session, SessionInvariantError,
    SessionMutationReader, StoredValue,
};
use std::collections::{HashMap, HashSet};

/// Complete stored envelope. Payload interpretation happens only after the
/// presence checks, preserving the upstream error precedence.
#[derive(Debug, Clone, PartialEq)]
pub struct CompleteLaneStorage {
    pub tip: StoredValue,
    pub configuration: StoredValue,
    pub lane_state: StoredValue,
}
#[derive(Debug, Clone, PartialEq)]
pub enum ClassifiedLaneStorage {
    Absent,
    Branch { tip: StoredValue },
    Lane(Box<CompleteLaneStorage>),
}
fn invariant(message: String) -> anyhow::Error {
    SessionInvariantError(message).into()
}
fn quote(value: &str) -> String {
    serde_json::to_string(value).expect("strings serialize")
}
fn classify_lane_storage(
    lane: &str,
    tip: Option<StoredValue>,
    configuration: Option<StoredValue>,
    state: Option<StoredValue>,
) -> anyhow::Result<ClassifiedLaneStorage> {
    if configuration.is_none() && state.is_none() {
        return Ok(match tip {
            Some(tip) => ClassifiedLaneStorage::Branch { tip },
            None => ClassifiedLaneStorage::Absent,
        });
    }
    let tip =
        tip.ok_or_else(|| invariant(format!("Lane {} is missing branch.tip", quote(lane))))?;
    let configuration = configuration
        .ok_or_else(|| invariant(format!("Lane {} is missing lane.config", quote(lane))))?;
    let lane_state =
        state.ok_or_else(|| invariant(format!("Lane {} is missing lane.state", quote(lane))))?;
    Ok(ClassifiedLaneStorage::Lane(Box::new(CompleteLaneStorage {
        tip,
        configuration,
        lane_state,
    })))
}

pub async fn read_lane_storage(
    reader: &dyn SessionMutationReader,
    lane: &str,
    context: Context,
) -> anyhow::Result<ClassifiedLaneStorage> {
    let tip_address = branch_tip(lane);
    let config_address = lane_config(lane);
    let state_address = lane_state(lane);
    let (tip, configuration, state) = futures::try_join!(
        reader.get_value(&tip_address, context.clone()),
        reader.get_value(&config_address, context.clone()),
        reader.get_value(&state_address, context)
    )?;
    classify_lane_storage(lane, tip, configuration, state)
}

/// An ordered map represented as pairs: preserves the upstream Map/Set order
/// rather than sorting lane names or exposing randomized HashMap iteration.
pub type RestoredLanes = Vec<(String, LaneState)>;

/// Restore every complete configured lane under one Session mutation barrier;
/// plain branches are not lanes and this function never commits or starts work.
pub async fn restore_session<S: Session + ?Sized>(
    session: &S,
    context: Context,
) -> anyhow::Result<RestoredLanes> {
    session
        .mutate(
            |reader, context| {
                Box::pin(async move {
                    let tip_prefix = branch_tip_inventory_prefix();
                    let config_prefix = lane_config("");
                    let state_prefix = lane_state("");
                    let (tips, configurations, states) = futures::try_join!(
                        reader.scan_values(&tip_prefix, context.clone()),
                        reader.scan_values(&config_prefix, context.clone()),
                        reader.scan_values(&state_prefix, context.clone())
                    )?;
                    let mut seen = HashSet::new();
                    let names: Vec<String> = tips
                        .iter()
                        .chain(configurations.iter())
                        .chain(states.iter())
                        .filter(|value| seen.insert(value.address.key.clone()))
                        .map(|value| value.address.key.clone())
                        .collect();
                    let tips: HashMap<_, _> = tips
                        .into_iter()
                        .map(|value| (value.address.key.clone(), value))
                        .collect();
                    let configs: HashMap<_, _> = configurations
                        .into_iter()
                        .map(|value| (value.address.key.clone(), value))
                        .collect();
                    let states: HashMap<_, _> = states
                        .into_iter()
                        .map(|value| (value.address.key.clone(), value))
                        .collect();
                    let mut restored = Vec::new();
                    for lane in names {
                        let stored = classify_lane_storage(
                            &lane,
                            tips.get(&lane).cloned(),
                            configs.get(&lane).cloned(),
                            states.get(&lane).cloned(),
                        )?;
                        if let ClassifiedLaneStorage::Lane(stored) = stored {
                            let state =
                                restore_lane_state(reader, &lane, &stored, context.clone()).await?;
                            restored.push((lane, state));
                        }
                    }
                    Ok(restored)
                })
            },
            context,
        )
        .await
}

pub async fn restore_lane<S: Session + ?Sized>(
    session: &S,
    lane: &str,
    context: Context,
) -> anyhow::Result<LaneState> {
    let lane = lane.to_owned();
    session
        .mutate(
            move |reader, context| {
                Box::pin(async move {
                    let stored = read_lane_storage(reader, &lane, context.clone()).await?;
                    match stored {
                        ClassifiedLaneStorage::Absent => Err(invariant(format!(
                            "Lane {} is missing branch.tip",
                            quote(&lane)
                        ))),
                        ClassifiedLaneStorage::Branch { .. } => Err(invariant(format!(
                            "Lane {} is missing lane.config",
                            quote(&lane)
                        ))),
                        ClassifiedLaneStorage::Lane(stored) => {
                            restore_lane_state(reader, &lane, &stored, context).await
                        }
                    }
                })
            },
            context,
        )
        .await
}

/// The upstream reachability rule is intent-sensitive, not just an at-prefix
/// check: summary boundaries carry operation ownership across dispatcher families.
pub fn state_matches_intent(intent: &OperationIntent, state: &OperationState) -> bool {
    match intent {
        OperationIntent::Compaction { .. } => state
            .phase
            .summary_task()
            .is_some_and(|task| matches!(task.boundary, ResultBoundary::Finish)),
        OperationIntent::Navigation {
            target_id,
            summarize,
            label,
            custom_instructions,
        } => {
            if let OperationPhase::NavigationReadyToCommit {
                target_id: current,
                label: current_label,
            } = &state.phase
            {
                return !summarize && current == target_id && current_label == label;
            }
            *summarize
                && state.phase.summary_task().is_some_and(|task| {
                    if let ResultBoundary::CommitNavigation {
                        target_id: current,
                        label: current_label,
                    } = &task.boundary
                    {
                        target_id.as_ref() == Some(current)
                            && current_label == label
                            && &task.custom_instructions == custom_instructions
                    } else {
                        false
                    }
                })
        }
        OperationIntent::Run { .. } => {
            !matches!(state.phase, OperationPhase::NavigationReadyToCommit { .. })
                && state.phase.summary_task().is_none_or(|task| {
                    matches!(task.boundary, ResultBoundary::ResumeCheckpoint { .. })
                })
        }
    }
}

pub async fn restore_lane_state(
    reader: &dyn SessionMutationReader,
    lane: &str,
    stored: &CompleteLaneStorage,
    context: Context,
) -> anyhow::Result<LaneState> {
    let durable: DurableLaneState = serde_json::from_value(stored.lane_state.value.clone())?;
    let operation = if let Some(operation_id) = durable.current_operation_id {
        let meta_address = operation_meta(&operation_id);
        let state_address = operation_state(&operation_id);
        let (meta, state) = futures::try_join!(
            reader.get_value(&meta_address, context.clone()),
            reader.get_value(&state_address, context)
        )?;
        let meta =
            meta.ok_or_else(|| invariant(format!("Operation {operation_id} is missing op.meta")))?;
        let state = state
            .ok_or_else(|| invariant(format!("Operation {operation_id} is missing op.state")))?;
        let meta: OperationMeta = serde_json::from_value(meta.value)?;
        let state: OperationState = serde_json::from_value(state.value)?;
        if meta.operation_id != operation_id {
            return Err(invariant(format!(
                "Operation {operation_id} metadata names operation {}",
                quote(&meta.operation_id)
            )));
        }
        if meta.lane != lane {
            return Err(invariant(format!(
                "Operation {operation_id} belongs to lane {}, not {}",
                quote(&meta.lane),
                quote(lane)
            )));
        }
        if !state_matches_intent(&meta.intent, &state) {
            return Err(invariant(format!(
                "Operation {operation_id} intent {} does not match state {}",
                meta.intent.kind(),
                state.at()
            )));
        }
        Some(Operation { meta, state })
    } else {
        None
    };
    Ok(LaneState {
        tip_id: serde_json::from_value(stored.tip.value.clone())?,
        configuration: serde_json::from_value(stored.configuration.value.clone())?,
        inbox: serde_json::from_value(durable.inbox)?,
        last_operation_id: durable.last_operation_id,
        operation,
    })
}
