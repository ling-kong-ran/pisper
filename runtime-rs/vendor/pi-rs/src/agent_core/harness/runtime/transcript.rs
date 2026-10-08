//! Effect-free transcript helpers from runtime/transcript.ts, including the
//! bounded context readers (`transcript.ts:50-84`) over the real Lane
//! capability.
use super::projection::{LaneQueuedItem, WriteKind};
use crate::agent_core::harness::context::Context;
use crate::agent_core::harness::runtime::drive_pass::Drive;
use crate::agent_core::harness::runtime::durable::OperationState;
use crate::agent_core::harness::runtime::lane::{ContinueOperationResult, Lane, OperationCommand};
use crate::agent_core::harness::session::{
    pending_entry, CommitResult, Entry, EntryType, InboxItem, InboxItemKind, NewEntry,
    PendingEntry, SessionInvariantError, SessionMutationReader, StorageBranchScan,
};
use crate::agent_core::types::AgentMessage;
use serde::{Deserialize, Serialize};

/// Upstream's object spread becomes an owned clone of staged entries. Existing
/// parent links are overwritten without modifying the caller's inputs.
pub fn chain_entries(parent_id: Option<&str>, items: &[NewEntry]) -> Vec<NewEntry> {
    let mut parent = parent_id.map(str::to_owned);
    items
        .iter()
        .map(|item| {
            let mut entry = item.clone();
            match &mut entry {
                NewEntry::Message { parent_id, .. }
                | NewEntry::Compaction { parent_id, .. }
                | NewEntry::BranchSummary { parent_id, .. }
                | NewEntry::Custom { parent_id, .. } => *parent_id = parent.take(),
            }
            parent = Some(item.id().to_owned());
            entry
        })
        .collect()
}

/// Native wire envelopes for this subset only, not the complete HarnessEvent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
#[allow(clippy::large_enum_variant)] // Preserve the existing owned Entry/AgentMessage API.
pub enum EntryLifecycleEvent {
    MessageStart {
        lane: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
        message: AgentMessage,
    },
    MessageEnd {
        lane: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
        message: AgentMessage,
        entry_id: String,
    },
    EntryAdded {
        lane: String,
        entry: Entry,
    },
}

pub fn entry_lifecycle_events(
    entry: &Entry,
    lane: &str,
    run_id: Option<&str>,
) -> Vec<EntryLifecycleEvent> {
    let mut events = Vec::with_capacity(3);
    if let Entry::Message { id, message, .. } = entry {
        events.push(EntryLifecycleEvent::MessageStart {
            lane: lane.into(),
            run_id: run_id.map(str::to_owned),
            message: message.clone(),
        });
        events.push(EntryLifecycleEvent::MessageEnd {
            lane: lane.into(),
            run_id: run_id.map(str::to_owned),
            message: message.clone(),
            entry_id: id.clone(),
        });
    }
    events.push(EntryLifecycleEvent::EntryAdded {
        lane: lane.into(),
        entry: entry.clone(),
    });
    events
}

/// The write offset is in transaction writes, not entry ordinal sequence values.
/// A malformed commit is rejected instead of manufacturing an undefined seq.
pub fn committed_entry_events(
    entries: &[NewEntry],
    commit: &CommitResult,
    lane: &str,
    run_id: Option<&str>,
    first_write_index: usize,
) -> anyhow::Result<Vec<EntryLifecycleEvent>> {
    let end = first_write_index
        .checked_add(entries.len())
        .ok_or_else(|| SessionInvariantError("Committed entry write offset overflow".into()))?;
    let seqs = commit.seqs.get(first_write_index..end).ok_or_else(|| {
        SessionInvariantError("Committed entries exceed the commit sequence range".into())
    })?;
    Ok(entries
        .iter()
        .zip(seqs)
        .flat_map(|(entry, seq)| {
            entry_lifecycle_events(
                &entry.clone().into_entry(*seq, commit.timestamp),
                lane,
                run_id,
            )
        })
        .collect())
}

pub async fn read_lane_queues(
    reader: &dyn SessionMutationReader,
    inbox: &[InboxItem],
    context: Context,
) -> anyhow::Result<Vec<LaneQueuedItem>> {
    futures::future::try_join_all(inbox.iter().map(|item| {
        let context = context.clone();
        async move {
            let stored = reader
                .get_value(&pending_entry(&item.entry_id), context)
                .await?
                .ok_or_else(|| {
                    SessionInvariantError(format!(
                        "Pending {} entry {} is missing its payload",
                        inbox_kind(item.kind),
                        item.entry_id
                    ))
                })?;
            let pending: PendingEntry = serde_json::from_value(stored.value)?;
            match pending {
                PendingEntry::Message { payload } => Ok(LaneQueuedItem::Message {
                    entry_id: item.entry_id.clone(),
                    kind: item.kind,
                    message: payload,
                }),
                PendingEntry::Custom {
                    custom_type,
                    payload,
                } => {
                    if item.kind != InboxItemKind::Write {
                        return Err(SessionInvariantError(format!(
                            "Pending {} entry {} is not a message",
                            inbox_kind(item.kind),
                            item.entry_id
                        ))
                        .into());
                    }
                    Ok(LaneQueuedItem::Custom {
                        entry_id: item.entry_id.clone(),
                        kind: WriteKind::Write,
                        custom_type,
                        data: payload,
                    })
                }
            }
        }
    }))
    .await
}
fn inbox_kind(kind: InboxItemKind) -> &'static str {
    match kind {
        InboxItemKind::Steer => "steer",
        InboxItemKind::FollowUp => "followUp",
        InboxItemKind::NextRun => "nextRun",
        InboxItemKind::Write => "write",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingMessage {
    pub entry_id: String,
    pub message: AgentMessage,
}

pub async fn read_pending_messages(
    reader: &dyn SessionMutationReader,
    ids: &[String],
    description: &str,
    context: Context,
) -> anyhow::Result<Vec<PendingMessage>> {
    futures::future::try_join_all(ids.iter().map(|id| {
        let context = context.clone();
        async move {
            let stored = reader.get_value(&pending_entry(id), context).await?;
            match stored.and_then(|value| (value.value["type"] == "message").then_some(value.value))
            {
                Some(value) => {
                    let PendingEntry::Message { payload } = serde_json::from_value(value)? else {
                        unreachable!("type was checked")
                    };
                    Ok(PendingMessage {
                        entry_id: id.clone(),
                        message: payload,
                    })
                }
                None => Err(SessionInvariantError(format!(
                    "{description} {id} is missing its message payload"
                ))
                .into()),
            }
        }
    }))
    .await
}

/// Upstream `readBoundedEntries` (`transcript.ts:50-67`): the branch path
/// around the running operation, from the tip back to the latest compaction
/// stop, returned oldest-first. Returns
/// [`ContinueOperationResult::CancelRequested`] once durable control is no
/// longer running.
pub async fn read_bounded_entries(
    lane: &Lane,
    drive: &Drive,
    _capability: &OperationState,
) -> anyhow::Result<ContinueOperationResult<Vec<Entry>>> {
    let context = drive.context().clone();
    let plan_context = context.clone();
    lane.continue_operation(
        move |state, _current, _meta, reader| {
            let context = plan_context.clone();
            Box::pin(async move {
                let Some(tip_id) = state.tip_id.clone() else {
                    anyhow::bail!("Run operation has no Branch tip");
                };
                let mut entries = reader
                    .scan_branch(
                        &StorageBranchScan {
                            stop_at_type: Some(EntryType::Compaction),
                            order: Some(
                                crate::agent_core::harness::session::BranchScanOrder::NewestFirst,
                            ),
                            start: tip_id,
                            ..Default::default()
                        },
                        context,
                    )
                    .await?;
                entries.reverse();
                Ok(OperationCommand::Return { result: entries })
            })
        },
        context,
    )
    .await
}

/// Upstream `readBoundedContext` (`transcript.ts:69-84`): the bounded branch
/// entries projected into provider messages with the lane's current entry
/// projectors.
pub async fn read_bounded_context(
    lane: &Lane,
    drive: &Drive,
    capability: &OperationState,
) -> anyhow::Result<ContinueOperationResult<Vec<AgentMessage>>> {
    let entries = read_bounded_entries(lane, drive, capability).await?;
    match entries {
        ContinueOperationResult::CancelRequested => Ok(ContinueOperationResult::CancelRequested),
        ContinueOperationResult::Result { value } => {
            let context = drive.context().clone();
            let projectors = lane.read_config().entry_projectors;
            let options = crate::agent_core::harness::session::SessionContextBuildOptions {
                entry_projectors: projectors,
            };
            let messages = crate::agent_core::harness::session::build_session_context(
                &value,
                Some(&options),
                context,
            )
            .await?;
            Ok(ContinueOperationResult::Result { value: messages })
        }
    }
}
