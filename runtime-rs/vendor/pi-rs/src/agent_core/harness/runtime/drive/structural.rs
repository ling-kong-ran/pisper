//! Port of `packages/agent/src/harness/runtime/drive/structural.ts` (1222
//! lines, sha256 `b5da9f9b8a6fe4bd8624eb2f833e02d955d229cbac3d0e8f9a2cda9a2c15154c`):
//! the durable structural drive — the preparation vocabulary
//! ([`DurableCompactionPreparation`] with its `kind: "compaction"` wire tag),
//! the two boundary prepares ([`prepare_compaction_threshold`],
//! [`prepare_overflow_compaction`]), and the generation/attempt/publish core:
//! decision hooks ([`run_structural_decision`]), attempt execution
//! ([`run_structural_generation`]), retry waits
//! ([`run_structural_retry_wait`]), orphan recovery
//! ([`recover_structural_generation`]), outcome publication
//! ([`publish_structural_outcome`]), and navigation commit
//! ([`commit_navigation`]).
//!
//! Disclosed substitutions and seams:
//! - **File-op arrays.** `durableFileOperations` upstream spreads the
//!   insertion-ordered sets into arrays; the Rust `HashSet` has no order, so
//!   the durable arrays are sorted for determinism (round-trips losslessly
//!   into sets).
//! - **`StructuralCancelled` through the `SummaryRequest` seam.** Upstream
//!   throws `StructuralCancelled` out of its request closure and
//!   `compactWithRequest` propagates it. The ported [`SummaryRequest`]
//!   boundary returns `AssistantMessage` and cannot fail, so cancellation
//!   (gate abort, cancelled intent, refused admission) records a flag,
//!   returns a synthetic aborted message, and
//!   [`perform_structural_attempt`] converts the flag into
//!   `cancel_requested` before the summary result is inspected. Non-abort
//!   hook/publish failures ride a hard-error slot and surface as `Err` after
//!   the summary call unwinds; later requests short-circuit so no extra
//!   provider call is started, matching the upstream throw.
//! - **Telemetry context.** `getTelemetryContext(context)` is deferred with
//!   the telemetry module; `SimpleStreamOptions` carries no such field (the
//!   compaction module made the same call for
//!   `createSummaryRequestOptions`).
//! - **Event field gaps in the fixed [`HarnessEvent`] union** (events.rs is
//!   an earlier landed slice and outside this port's scope):
//!   `retry_scheduled` cannot carry upstream's `recovery: true` flag, failed
//!   `compaction_end` cannot carry upstream's `error` payload, and a declined
//!   `navigation_end` (`status: "declined"`) is outside the ported
//!   `RunEndStatus` union — the port emits `aborted`. Durable records keep
//!   the exact upstream statuses; the deltas are telemetry-shape only.
//! - **`finishRunBoundary` pending-event order.** Upstream prepends
//!   `pendingEvents` before its own events; the landed `finish_run_boundary`
//!   chains them last. The port passes the declined `compaction_end` through
//!   that landed seam unchanged (see the test disclosure).
//! - **Root navigation tip.** `commitNavigation` with a `null` target writes
//!   the durable `branch_tip` as null, but `LanePatch` cannot express a null
//!   in-memory tip (`None` means "no change"), so the process-local tip stays
//!   put for root navigations only.
//! - **Clock.** `Date.now()` reads map to [`crate::ai::now_ms`]; the
//!   `notBefore` wait reuses the `runRetryWait` admit/error dance from
//!   generation.rs.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use crate::agent_core::harness::compaction::{
    compact_with_request, generate_branch_summary_with_request, prepare_compaction, should_compact,
    BranchPreparation, BranchSummaryResult, CompactGenerationOptions, CompactResult,
    CompactionPreparation, PreparedBranchSummaryOptions, SummaryRequest,
};
use crate::agent_core::harness::config::CompactionSettings;
use crate::agent_core::harness::context::{with_abort_signal, Context};
use crate::agent_core::harness::execution::effect_gate::{AbortRequested, GateRejection};
use crate::agent_core::harness::hooks::{
    apply_stream_options_patch, BeforeCompactionEvent, BeforeNavigationEvent, BeforePayloadEvent,
    BeforeRequestEvent, HookEvent, HookInvocation, HookName, HookResult, RequestStep,
};
use crate::agent_core::harness::runtime::drive::boundary::{
    assistant_ready_at_boundary, boundary_placement_events, finish_run_boundary,
    normalized_retry_policy, plan_boundary_inbox,
};
use crate::agent_core::harness::runtime::drive::retry::{
    retry_not_before, wait_until, RetryDelayPolicy,
};
use crate::agent_core::harness::runtime::drive::terminal::{
    operation_cleanup_writes, operation_result_record,
};
use crate::agent_core::harness::runtime::drive_pass::WaitingReason;
use crate::agent_core::harness::runtime::drive_pass::{Drive, DriveOutcome, ProcedureResult};
use crate::agent_core::harness::runtime::durable::{
    Continuation, OperationPhase, OperationState, ResultBoundary, RetryWait, SummaryContext,
    SummaryReason, SummaryRequest as DurableSummaryRequest, SummaryTask,
};
use crate::agent_core::harness::runtime::events::{
    CompactionEndStatus, HarnessEvent, RunEndStatus,
};
use crate::agent_core::harness::runtime::lane::{
    ContinueOperationResult, Lane, LanePatch, OperationCommand,
};
use crate::agent_core::harness::runtime::transcript::{
    committed_entry_events, read_bounded_entries,
};
use crate::agent_core::harness::session::commit::{insert_entry, insert_usage};
use crate::agent_core::harness::session::types::{
    CommitResult, NewEntry, NewUsageRow, OperationError, TerminalStatus, UsageRow, Write,
};
use crate::agent_core::harness::session::values::{
    branch_tip, entry_label, operation_preparation, set_value,
};
use crate::agent_core::harness::session::Session as _;
use crate::agent_core::harness::session::{Control, SessionInvariantError};
use crate::agent_core::harness::types::AgentHarnessStreamOptions;
use crate::agent_core::types::AgentMessage;
use crate::ai::models::ModelsSimpleStreamOptions;
use crate::ai::retry::{is_retryable_assistant_error, retry_delay_ms};
use crate::ai::transcript::Context as AiContext;
use crate::ai::types::message::AssistantMessage;
use crate::ai::types::model::Model;
use crate::ai::types::options::{DeferredFlag, SimpleStreamOptions};
use crate::ai::types::primitives::{CacheRetention, Usage};

/// Upstream `DurableFileOperations` — the staged file-operation record,
/// serialized as sorted arrays.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DurableFileOperations {
    pub read: Vec<String>,
    pub written: Vec<String>,
    pub edited: Vec<String>,
}

/// Upstream `Extract<DurableStructuralPreparation, { kind: "compaction" }>`:
/// the durable compaction preparation with its discriminant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename = "compaction", rename_all = "camelCase")]
pub struct DurableCompactionPreparation {
    pub messages_to_summarize: Vec<AgentMessage>,
    pub turn_prefix_messages: Vec<AgentMessage>,
    pub retained_tail: Vec<AgentMessage>,
    pub is_split_turn: bool,
    pub tokens_before: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_summary: Option<String>,
    pub file_ops: DurableFileOperations,
    pub settings: CompactionSettings,
}

/// Upstream `Extract<DurableStructuralPreparation, { kind: "branch_summary" }>`
/// (`structural.ts:91-100`): the durable branch-summary preparation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename = "branch_summary", rename_all = "camelCase")]
pub struct DurableBranchPreparation {
    pub messages: Vec<AgentMessage>,
    pub file_ops: DurableFileOperations,
    pub total_tokens: u64,
}

/// Upstream `DurableStructuralPreparation` (`session/types.ts`), discriminated
/// by the stored `kind` field exactly like the upstream consumers.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum DurableStructuralPreparation {
    Compaction(DurableCompactionPreparation),
    BranchSummary(DurableBranchPreparation),
}

impl DurableStructuralPreparation {
    /// The stored `kind` discriminant (`"compaction" | "branch_summary"`).
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Compaction(_) => "compaction",
            Self::BranchSummary(_) => "branch_summary",
        }
    }
}

/// Parse a stored preparation value by its `kind` field (upstream consumers
/// branch on `stored.value.kind`).
fn parse_durable_structural_preparation(
    value: serde_json::Value,
) -> anyhow::Result<DurableStructuralPreparation> {
    match value.get("kind").and_then(|kind| kind.as_str()) {
        Some("compaction") => Ok(DurableStructuralPreparation::Compaction(
            serde_json::from_value(value)?,
        )),
        Some("branch_summary") => Ok(DurableStructuralPreparation::BranchSummary(
            serde_json::from_value(value)?,
        )),
        _ => Err(invariant(
            "Stored structural preparation is missing its kind discriminant".to_string(),
        )),
    }
}

/// The `structural.ts` prepare result: a fresh task id plus its durable
/// preparation.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StructuralPreparation {
    pub task_id: String,
    pub preparation: DurableCompactionPreparation,
}

/// Upstream `durableFileOperations`: the staged file-operation record as
/// sorted arrays (see the module docs for the order substitution).
fn durable_file_operations(
    file_ops: &crate::agent_core::harness::compaction::FileOperations,
) -> DurableFileOperations {
    fn sorted(set: &std::collections::HashSet<String>) -> Vec<String> {
        let mut items: Vec<String> = set.iter().cloned().collect();
        items.sort();
        items
    }
    DurableFileOperations {
        read: sorted(&file_ops.read),
        written: sorted(&file_ops.written),
        edited: sorted(&file_ops.edited),
    }
}

/// Upstream `durableCompactionPreparation` (`structural.ts:75-90`).
pub fn durable_compaction_preparation(
    preparation: &CompactionPreparation,
) -> DurableCompactionPreparation {
    DurableCompactionPreparation {
        messages_to_summarize: preparation.messages_to_summarize.clone(),
        turn_prefix_messages: preparation.turn_prefix_messages.clone(),
        retained_tail: preparation.retained_tail.clone(),
        is_split_turn: preparation.is_split_turn,
        tokens_before: preparation.tokens_before,
        previous_summary: preparation.previous_summary.clone(),
        file_ops: durable_file_operations(&preparation.file_ops),
        settings: preparation.settings,
    }
}

/// Upstream `durableBranchPreparation` (`structural.ts:91-100`).
pub fn durable_branch_preparation(preparation: &BranchPreparation) -> DurableBranchPreparation {
    DurableBranchPreparation {
        messages: preparation.messages.clone(),
        file_ops: durable_file_operations(&preparation.file_ops),
        total_tokens: preparation.total_tokens,
    }
}

/// Upstream `fileOperations` (`structural.ts:102-108`): the durable arrays
/// back into sets.
fn file_operations(
    durable: &DurableFileOperations,
) -> crate::agent_core::harness::compaction::FileOperations {
    crate::agent_core::harness::compaction::FileOperations {
        read: durable.read.iter().cloned().collect(),
        written: durable.written.iter().cloned().collect(),
        edited: durable.edited.iter().cloned().collect(),
    }
}

/// Upstream `compactionPreparation` (`structural.ts:110-123`).
fn live_compaction_preparation(durable: DurableCompactionPreparation) -> CompactionPreparation {
    let DurableCompactionPreparation {
        messages_to_summarize,
        turn_prefix_messages,
        retained_tail,
        is_split_turn,
        tokens_before,
        previous_summary,
        file_ops,
        settings,
    } = durable;
    CompactionPreparation {
        messages_to_summarize,
        turn_prefix_messages,
        retained_tail,
        is_split_turn,
        tokens_before,
        previous_summary,
        file_ops: file_operations(&file_ops),
        settings,
    }
}

/// Upstream `branchPreparation` (`structural.ts:125-133`).
fn live_branch_preparation(durable: DurableBranchPreparation) -> BranchPreparation {
    let DurableBranchPreparation {
        messages,
        file_ops,
        total_tokens,
    } = durable;
    BranchPreparation {
        messages,
        file_ops: file_operations(&file_ops),
        total_tokens,
    }
}

/// Upstream `CompactionPreparation | BranchPreparation`, the live
/// (set-backed) preparation union.
#[derive(Debug, Clone)]
pub enum LivePreparation {
    Compaction(CompactionPreparation),
    BranchSummary(BranchPreparation),
}

fn invariant(message: String) -> anyhow::Error {
    anyhow::Error::new(SessionInvariantError(message))
}

/// Upstream `summaryKind` (`structural.ts:135-137`).
fn summary_kind(task: &SummaryTask) -> &'static str {
    match task.boundary {
        ResultBoundary::CommitNavigation { .. } => "branch_summary",
        _ => "compaction",
    }
}

/// Upstream `compactionReason` (`structural.ts:139-143`).
fn compaction_reason(task: &SummaryTask) -> anyhow::Result<SummaryReason> {
    if let Some(reason) = task.reason {
        return Ok(reason);
    }
    if matches!(task.boundary, ResultBoundary::Finish) {
        return Ok(SummaryReason::Manual);
    }
    Err(invariant(format!(
        "In-run compaction task {} is missing its reason",
        task.task_id
    )))
}

/// Upstream `navigationBoundary` (`structural.ts:145-150`): the
/// `(targetId, label)` pair of a navigation task.
fn navigation_boundary(task: &SummaryTask) -> anyhow::Result<(String, Option<String>)> {
    match &task.boundary {
        ResultBoundary::CommitNavigation { target_id, label } => {
            Ok((target_id.clone(), label.clone()))
        }
        _ => Err(invariant(format!(
            "Summary task {} is not a navigation",
            task.task_id
        ))),
    }
}

/// Upstream `readStructuralPreparation` (`structural.ts:152-186`): consume the
/// durable preparation of a deciding summary task.
async fn read_structural_preparation(
    lane: &Arc<Lane>,
    drive: &Drive,
    _deciding: &OperationState,
) -> anyhow::Result<ContinueOperationResult<LivePreparation>> {
    let operation_id = drive.operation_id().to_owned();
    let plan_context = drive.context().clone();
    let context = plan_context.clone();
    lane.continue_operation(
        move |_state, current, _meta, reader| {
            let operation_id = operation_id.clone();
            let context = context.clone();
            Box::pin(async move {
                let Some(task) = current.phase.summary_task() else {
                    anyhow::bail!("Structural preparation requires a summary operation");
                };
                let expected = summary_kind(task);
                let stored = reader
                    .get_value(
                        &operation_preparation(&operation_id, &task.task_id),
                        context.clone(),
                    )
                    .await?;
                let kind_of = stored
                    .as_ref()
                    .and_then(|stored| stored.value.get("kind"))
                    .and_then(|kind| kind.as_str());
                if kind_of != Some(expected) {
                    return Err(invariant(format!(
                        "Structural task {} is missing its {} preparation",
                        task.task_id, expected
                    )));
                }
                let stored = stored.expect("kind checked above");
                let durable = parse_durable_structural_preparation(stored.value)?;
                if let ResultBoundary::CommitNavigation { target_id, .. } = &task.boundary {
                    let entries = reader
                        .get_entries(std::slice::from_ref(target_id), context.clone())
                        .await?;
                    if !entries.contains_key(target_id) {
                        return Err(invariant(format!(
                            "Navigation target {} is missing",
                            target_id
                        )));
                    }
                }
                let preparation = match durable {
                    DurableStructuralPreparation::Compaction(value) => {
                        LivePreparation::Compaction(live_compaction_preparation(value))
                    }
                    DurableStructuralPreparation::BranchSummary(value) => {
                        LivePreparation::BranchSummary(live_branch_preparation(value))
                    }
                };
                Ok(OperationCommand::Return {
                    result: preparation,
                })
            })
        },
        plan_context,
    )
    .await
}

/// Upstream `summaryContext` (`structural.ts:188-199`).
fn summary_context(
    lane: &Lane,
    result_entry_id: String,
    configuration: crate::agent_core::harness::session::LaneConfiguration,
) -> SummaryContext {
    let mut stream_options: AgentHarnessStreamOptions = lane.read_config().stream_options;
    stream_options.deferred = Some(DeferredFlag::Bool(false));
    SummaryContext {
        result_entry_id,
        configuration,
        stream_options,
        retry_policy: normalized_retry_policy(lane),
    }
}

/// Upstream `usageEvent` (`structural.ts:201-208`).
fn usage_event(
    row: &NewUsageRow,
    write_index: usize,
    commit: &CommitResult,
    lane: &str,
) -> anyhow::Result<HarnessEvent> {
    let Some(seq) = commit.seqs.get(write_index).copied() else {
        anyhow::bail!("commit sequence missing for usage row");
    };
    Ok(HarnessEvent::Usage {
        lane: lane.to_string(),
        row: UsageRow {
            id: row.id.clone(),
            seq,
            usage: row.usage,
            entry_id: row.entry_id.clone(),
            adjustment: row.adjustment,
            details: row.details.clone(),
        },
        totals: commit.stats.usage,
    })
}

/// Upstream `operationError` (`structural.ts:210-212`).
fn operation_error(code: &str, message: impl Into<String>) -> OperationError {
    OperationError {
        code: code.to_string(),
        message: message.into(),
        details: None,
    }
}

/// Upstream `StructuralOutcome` (`structural.ts:214-219`).
#[derive(Debug, Clone)]
enum StructuralOutcome {
    Compaction {
        result_entry_id: String,
        result: CompactResult,
        from_hook: bool,
    },
    BranchSummary {
        result_entry_id: String,
        result: BranchSummaryResult,
        from_hook: bool,
    },
    Declined,
    Failed {
        error: OperationError,
    },
}

impl StructuralOutcome {
    fn kind_name(&self) -> &'static str {
        match self {
            Self::Compaction { .. } => "compaction",
            Self::BranchSummary { .. } => "branch_summary",
            Self::Declined => "declined",
            Self::Failed { .. } => "failed",
        }
    }

    fn usage(&self) -> Option<Usage> {
        match self {
            Self::Compaction { result, .. } => result.usage,
            Self::BranchSummary { result, .. } => result.usage,
            _ => None,
        }
    }
}

/// Upstream `ProcedureResult | BoundaryFinishPending`
/// (`structural.ts:220`).
enum StructuralPublication {
    // Keep this internal sum type small even when a result owns ordered JSON.
    Procedure(Box<ProcedureResult>),
    FinishPending { entry_ids: Vec<String> },
}

impl StructuralPublication {
    fn procedure(result: ProcedureResult) -> Self {
        Self::Procedure(Box::new(result))
    }
}

/// One deferred base-event builder; `terminal_compaction_ended_at` carries the
/// finish-boundary record timestamp the upstream closure read through the
/// `terminalCompactionEndedAt` let-binding. Each builder fires exactly once at
/// materialization, so the type is `FnOnce`.
type StructuralBaseEvent =
    Box<dyn FnOnce(&CommitResult, Option<i64>) -> anyhow::Result<Vec<HarnessEvent>> + Send>;

fn flatten_base_events(
    base_events: Vec<StructuralBaseEvent>,
    commit: &CommitResult,
    terminal_compaction_ended_at: Option<i64>,
) -> anyhow::Result<Vec<HarnessEvent>> {
    let mut events = Vec::new();
    for build in base_events {
        events.extend(build(commit, terminal_compaction_ended_at)?);
    }
    Ok(events)
}

/// Upstream `publishStructuralOutcome` (`structural.ts:222-577`): publish one
/// structural outcome through the capability's next operation transaction.
#[allow(clippy::too_many_lines)]
async fn publish_structural_outcome(
    lane: &Arc<Lane>,
    drive: &Drive,
    capability: &OperationState,
    outcome: StructuralOutcome,
) -> anyhow::Result<ProcedureResult> {
    let hook_usage_id = match &outcome {
        StructuralOutcome::Compaction {
            from_hook: true, ..
        }
        | StructuralOutcome::BranchSummary {
            from_hook: true, ..
        } if outcome.usage().is_some() => Some(lane.session().id_generator().next(None)),
        _ => None,
    };
    let operation_id = drive.operation_id().to_owned();
    let plan_context = drive.context().clone();
    let context = plan_context.clone();
    let planner_lane = Arc::clone(lane);
    let outcome_for_planner = outcome.clone();
    let published = lane
        .continue_operation(
            move |state, current, meta, reader| {
                let lane = Arc::clone(&planner_lane);
                let outcome = outcome_for_planner.clone();
                let operation_id = operation_id.clone();
                let context = context.clone();
                let hook_usage_id = hook_usage_id.clone();
                let source_tip_id = meta.source_tip_id.clone();
                Box::pin(async move {
                    let Some(task) = current.phase.summary_task() else {
                        anyhow::bail!("Structural publication requires a summary operation");
                    };
                    let task = task.clone();
                    let expected = summary_kind(&task);
                    if matches!(
                        outcome,
                        StructuralOutcome::Compaction { .. }
                            | StructuralOutcome::BranchSummary { .. }
                    ) && outcome.kind_name() != expected
                    {
                        return Err(invariant(format!(
                            "Structural {} result does not match {} task {}",
                            outcome.kind_name(),
                            expected,
                            task.task_id
                        )));
                    }

                    let mut writes: Vec<Write> = Vec::new();
                    let mut base_events: Vec<StructuralBaseEvent> = Vec::new();
                    let mut terminal_tip_id = state.tip_id.clone();
                    if let Some(usage_id) = &hook_usage_id {
                        let usage = outcome.usage().ok_or_else(|| {
                            invariant("Hook usage id exists without structural usage".to_string())
                        })?;
                        let row = NewUsageRow {
                            id: usage_id.clone(),
                            usage,
                            entry_id: None,
                            adjustment: false,
                            details: None,
                        };
                        let write_index = writes.len();
                        writes.push(insert_usage(row.clone()));
                        let lane_name = lane.name().to_owned();
                        base_events.push(Box::new(move |commit, _terminal| {
                            Ok(vec![usage_event(&row, write_index, commit, &lane_name)?])
                        }));
                    }

                    if let StructuralOutcome::Compaction {
                        result_entry_id,
                        result,
                        from_hook,
                    } = &outcome
                    {
                        let entry = NewEntry::Compaction {
                            id: result_entry_id.clone(),
                            parent_id: state.tip_id.clone(),
                            summary: result.summary.clone(),
                            retained_tail: result.retained_tail.clone(),
                            tokens_before: result.tokens_before as i64,
                            details: match &result.details {
                                Some(details) => Some(serde_json::to_value(details)?),
                                None => None,
                            },
                            usage: result.usage,
                            from_hook: *from_hook,
                        };
                        let entry_write_index = writes.len();
                        writes.push(insert_entry(entry.clone()));
                        writes.push(set_value(
                            &branch_tip(lane.name()),
                            serde_json::Value::String(result_entry_id.clone()),
                        ));
                        terminal_tip_id = Some(result_entry_id.clone());
                        let lane_name = lane.name().to_owned();
                        let run_id = operation_id.clone();
                        base_events.push(Box::new(move |commit, _terminal| {
                            Ok(committed_entry_events(
                                std::slice::from_ref(&entry),
                                commit,
                                &lane_name,
                                Some(&run_id),
                                entry_write_index,
                            )?
                            .into_iter()
                            .map(HarnessEvent::from)
                            .collect())
                        }));
                    } else if let StructuralOutcome::BranchSummary {
                        result_entry_id,
                        result,
                        from_hook,
                    } = &outcome
                    {
                        let (target_id, label) = navigation_boundary(&task)?;
                        let entry = NewEntry::BranchSummary {
                            id: result_entry_id.clone(),
                            parent_id: Some(target_id.clone()),
                            from_id: source_tip_id.clone(),
                            summary: result.summary.clone(),
                            details: Some(serde_json::json!({
                                "readFiles": result.read_files,
                                "modifiedFiles": result.modified_files,
                            })),
                            usage: result.usage,
                            from_hook: *from_hook,
                        };
                        writes.push(set_value(
                            &branch_tip(lane.name()),
                            serde_json::Value::String(target_id.clone()),
                        ));
                        let entry_write_index = writes.len();
                        writes.push(insert_entry(entry.clone()));
                        writes.push(set_value(
                            &branch_tip(lane.name()),
                            serde_json::Value::String(result_entry_id.clone()),
                        ));
                        if let Some(label) = &label {
                            writes.push(set_value(
                                &entry_label(&target_id),
                                serde_json::Value::String(label.clone()),
                            ));
                        }
                        terminal_tip_id = Some(result_entry_id.clone());
                        let lane_name = lane.name().to_owned();
                        let run_id = operation_id.clone();
                        base_events.push(Box::new(move |commit, _terminal| {
                            Ok(committed_entry_events(
                                std::slice::from_ref(&entry),
                                commit,
                                &lane_name,
                                Some(&run_id),
                                entry_write_index,
                            )?
                            .into_iter()
                            .map(HarnessEvent::from)
                            .collect())
                        }));
                    }

                    let attempt = match &current.phase {
                        OperationPhase::SummaryReady { next_attempt, .. } => Some(*next_attempt),
                        OperationPhase::SummaryEffectPending { attempt, .. } => Some(*attempt),
                        _ => None,
                    };
                    if attempt.is_some_and(|attempt| attempt > 1) {
                        let attempt = attempt.expect("checked above");
                        let success = matches!(
                            outcome,
                            StructuralOutcome::Compaction { .. }
                                | StructuralOutcome::BranchSummary { .. }
                        );
                        let final_error = match &outcome {
                            StructuralOutcome::Failed { error } => Some(error.message.clone()),
                            _ => None,
                        };
                        let lane_name = lane.name().to_owned();
                        let run_id = operation_id.clone();
                        let step = task.task_id.clone();
                        base_events.push(Box::new(move |_commit, _terminal| {
                            Ok(vec![HarnessEvent::RetryEnd {
                                lane: lane_name,
                                run_id,
                                step,
                                attempt,
                                success,
                                final_error,
                            }])
                        }));
                    }
                    if let StructuralOutcome::Compaction {
                        result_entry_id, ..
                    } = &outcome
                    {
                        let reason = compaction_reason(&task)?;
                        let lane_name = lane.name().to_owned();
                        let run_id = operation_id.clone();
                        let entry_id = result_entry_id.clone();
                        base_events.push(Box::new(move |commit, terminal| {
                            Ok(vec![HarnessEvent::CompactionEnd {
                                lane: lane_name,
                                run_id,
                                reason,
                                status: CompactionEndStatus::Completed,
                                entry_id: Some(entry_id),
                                ended_at: terminal.unwrap_or(commit.timestamp),
                            }])
                        }));
                    }

                    match &task.boundary {
                        ResultBoundary::ResumeCheckpoint { resume_after } => {
                            if matches!(outcome, StructuralOutcome::BranchSummary { .. }) {
                                return Err(invariant(
                                    "Run compaction boundary received a branch summary".to_string(),
                                ));
                            }
                            let declined_threshold = matches!(outcome, StructuralOutcome::Declined)
                                && task.reason == Some(SummaryReason::Threshold);
                            if matches!(outcome, StructuralOutcome::Compaction { .. })
                                || declined_threshold
                            {
                                if terminal_tip_id.is_none() {
                                    return Err(invariant(
                                        "Run compaction has no Branch tip".to_string(),
                                    ));
                                }
                                let continuation = &resume_after.continuation;
                                let placement = plan_boundary_inbox(
                                    &lane,
                                    state,
                                    &current.scope,
                                    reader,
                                    terminal_tip_id.clone(),
                                    matches!(outcome, StructuralOutcome::Declined)
                                        && matches!(continuation, Continuation::MayFinish { .. }),
                                    &context,
                                )
                                .await?;
                                if matches!(outcome, StructuralOutcome::Declined)
                                    && placement.trigger_entry_id.is_none()
                                    && matches!(continuation, Continuation::MayFinish { .. })
                                {
                                    return Ok(OperationCommand::Return {
                                        result: StructuralPublication::FinishPending {
                                            entry_ids: placement
                                                .entries
                                                .iter()
                                                .map(|entry| entry.id().to_string())
                                                .collect(),
                                        },
                                    });
                                }
                                let placement_write_index = writes.len();
                                writes.extend(placement.writes.iter().cloned());
                                let operation_state = if placement.trigger_entry_id.is_some()
                                    || matches!(continuation, Continuation::NeedAssistant { .. })
                                {
                                    let overflow_recovery_used = match (
                                        placement.trigger_entry_id.is_none(),
                                        continuation,
                                    ) {
                                        (
                                            true,
                                            Continuation::NeedAssistant {
                                                overflow_recovery_used,
                                            },
                                        ) => *overflow_recovery_used,
                                        _ => false,
                                    };
                                    assistant_ready_at_boundary(
                                        &lane,
                                        state,
                                        &current.scope,
                                        placement.trigger_entry_id.clone().unwrap_or_else(|| {
                                            resume_after.trigger_entry_id.clone()
                                        }),
                                        overflow_recovery_used,
                                    )
                                } else {
                                    OperationState {
                                        scope: current.scope.clone(),
                                        phase: OperationPhase::Checkpoint {
                                            checkpoint: resume_after.clone(),
                                        },
                                    }
                                };
                                let declined = matches!(outcome, StructuralOutcome::Declined);
                                let lane_name = lane.name().to_owned();
                                let run_id = operation_id.clone();
                                return Ok(OperationCommand::Commit {
                                    writes,
                                    operation_state,
                                    lane: Some(LanePatch {
                                        tip_id: placement.tip_id.clone(),
                                        configuration: None,
                                        inbox: Some(placement.inbox.clone()),
                                    }),
                                    materialize: Box::new(|_commit: &CommitResult| {
                                        StructuralPublication::procedure(ProcedureResult::Continue)
                                    }),
                                    events: Some(Box::new(move |commit: &CommitResult| {
                                        let mut events = if declined {
                                            vec![HarnessEvent::CompactionEnd {
                                                lane: lane_name.clone(),
                                                run_id: run_id.clone(),
                                                reason: SummaryReason::Threshold,
                                                status: CompactionEndStatus::Declined,
                                                entry_id: None,
                                                ended_at: commit.timestamp,
                                            }]
                                        } else {
                                            flatten_base_events(base_events, commit, None)?
                                        };
                                        events.extend(boundary_placement_events(
                                            &placement,
                                            commit,
                                            placement_write_index,
                                            &lane_name,
                                            &run_id,
                                        )?);
                                        Ok(events)
                                    })),
                                });
                            }
                            let Some(tip_id) = state.tip_id.clone() else {
                                return Err(invariant("Failed run has no Branch tip".to_string()));
                            };
                            let error = match &outcome {
                                StructuralOutcome::Declined => operation_error(
                                    "compaction_declined",
                                    "Overflow compaction was declined",
                                ),
                                StructuralOutcome::Failed { error } => error.clone(),
                                _ => unreachable!("compaction outcomes returned above"),
                            };
                            let cleanup = operation_cleanup_writes(
                                reader,
                                &operation_id,
                                current,
                                context.clone(),
                            )
                            .await?;
                            let record = operation_result_record(
                                meta,
                                TerminalStatus::Failed,
                                state.tip_id.clone(),
                                Some(error.clone()),
                            )?;
                            let reason = compaction_reason(&task)?;
                            // Upstream attaches `error` to the failed
                            // compaction_end; the fixed HarnessEvent union has
                            // no such field (module docs).
                            let compaction_end = HarnessEvent::CompactionEnd {
                                lane: lane.name().to_owned(),
                                run_id: operation_id.clone(),
                                reason,
                                status: match &outcome {
                                    StructuralOutcome::Declined => CompactionEndStatus::Declined,
                                    _ => CompactionEndStatus::Failed,
                                },
                                entry_id: None,
                                ended_at: record.ended_at,
                            };
                            let record_for_materialize = record.clone();
                            let record_ended_at = record.ended_at;
                            let run_error = error.clone();
                            let lane_name = lane.name().to_owned();
                            let run_id = operation_id.clone();
                            let source = source_tip_id.clone();
                            Ok(OperationCommand::Finish {
                                writes: {
                                    writes.extend(cleanup);
                                    writes
                                },
                                record,
                                lane: None,
                                materialize: Box::new(move |_commit: &CommitResult| {
                                    StructuralPublication::procedure(ProcedureResult::Settled {
                                        outcome: record_for_materialize,
                                    })
                                }),
                                events: Some(Box::new(move |commit: &CommitResult| {
                                    let mut events =
                                        flatten_base_events(base_events, commit, None)?;
                                    events.push(compaction_end);
                                    events.push(HarnessEvent::RunEnd {
                                        lane: lane_name,
                                        run_id,
                                        status: RunEndStatus::Failed,
                                        error: Some(run_error),
                                        from_tip_id: source,
                                        tip_id: Some(tip_id),
                                        ended_at: record_ended_at,
                                    });
                                    Ok(events)
                                })),
                            })
                        }
                        ResultBoundary::Finish => {
                            if matches!(outcome, StructuralOutcome::BranchSummary { .. }) {
                                return Err(invariant(
                                    "Compaction finish boundary received a branch summary"
                                        .to_string(),
                                ));
                            }
                            if !matches!(outcome, StructuralOutcome::Compaction { .. })
                                && state.tip_id.is_none()
                            {
                                return Err(invariant(
                                    "Standalone compaction has no Branch tip".to_string(),
                                ));
                            }
                            let error = match &outcome {
                                StructuralOutcome::Failed { error } => Some(error.clone()),
                                _ => None,
                            };
                            let status = match &outcome {
                                StructuralOutcome::Declined => TerminalStatus::Declined,
                                StructuralOutcome::Failed { .. } => TerminalStatus::Failed,
                                _ => TerminalStatus::Completed,
                            };
                            let cleanup = operation_cleanup_writes(
                                reader,
                                &operation_id,
                                current,
                                context.clone(),
                            )
                            .await?;
                            let record = operation_result_record(
                                meta,
                                status,
                                terminal_tip_id.clone(),
                                error,
                            )?;
                            // Upstream assigns terminalCompactionEndedAt = record.endedAt
                            // before materializing, so a completed compaction_end on this
                            // boundary stamps the record time instead of commit.timestamp.
                            let terminal_compaction_ended_at = record.ended_at;
                            let compaction_end =
                                if matches!(outcome, StructuralOutcome::Compaction { .. }) {
                                    None
                                } else {
                                    // Upstream hard-codes reason "manual" (finish-boundary
                                    // compaction tasks are always manual).
                                    Some(HarnessEvent::CompactionEnd {
                                        lane: lane.name().to_owned(),
                                        run_id: operation_id.clone(),
                                        reason: SummaryReason::Manual,
                                        status: match &outcome {
                                            StructuralOutcome::Declined => {
                                                CompactionEndStatus::Declined
                                            }
                                            _ => CompactionEndStatus::Failed,
                                        },
                                        entry_id: None,
                                        ended_at: record.ended_at,
                                    })
                                };
                            let lane_patch =
                                if matches!(outcome, StructuralOutcome::Compaction { .. }) {
                                    Some(LanePatch {
                                        tip_id: terminal_tip_id.clone(),
                                        configuration: None,
                                        inbox: None,
                                    })
                                } else {
                                    None
                                };
                            let record_for_materialize = record.clone();
                            Ok(OperationCommand::Finish {
                                writes: {
                                    writes.extend(cleanup);
                                    writes
                                },
                                record,
                                lane: lane_patch,
                                materialize: Box::new(move |_commit: &CommitResult| {
                                    StructuralPublication::procedure(ProcedureResult::Settled {
                                        outcome: record_for_materialize,
                                    })
                                }),
                                events: Some(Box::new(move |commit: &CommitResult| {
                                    let mut events = flatten_base_events(
                                        base_events,
                                        commit,
                                        Some(terminal_compaction_ended_at),
                                    )?;
                                    events.extend(compaction_end);
                                    Ok(events)
                                })),
                            })
                        }
                        ResultBoundary::CommitNavigation { .. } => {
                            if matches!(outcome, StructuralOutcome::Compaction { .. }) {
                                return Err(invariant(
                                    "Navigation boundary received a compaction result".to_string(),
                                ));
                            }
                            let error = match &outcome {
                                StructuralOutcome::Failed { error } => Some(error.clone()),
                                _ => None,
                            };
                            let status = match &outcome {
                                StructuralOutcome::Declined => TerminalStatus::Declined,
                                StructuralOutcome::Failed { .. } => TerminalStatus::Failed,
                                _ => TerminalStatus::Completed,
                            };
                            let cleanup = operation_cleanup_writes(
                                reader,
                                &operation_id,
                                current,
                                context.clone(),
                            )
                            .await?;
                            let record = operation_result_record(
                                meta,
                                status,
                                terminal_tip_id.clone(),
                                error,
                            )?;
                            let navigation_end = match &outcome {
                                StructuralOutcome::BranchSummary { .. } => {
                                    HarnessEvent::NavigationEnd {
                                        lane: lane.name().to_owned(),
                                        run_id: operation_id.clone(),
                                        status: RunEndStatus::Completed,
                                        from_tip_id: source_tip_id.clone(),
                                        tip_id: terminal_tip_id.clone(),
                                        ended_at: record.ended_at,
                                    }
                                }
                                // Upstream status "declined" is not in the ported
                                // RunEndStatus union (module docs).
                                StructuralOutcome::Declined => HarnessEvent::NavigationEnd {
                                    lane: lane.name().to_owned(),
                                    run_id: operation_id.clone(),
                                    status: RunEndStatus::Aborted,
                                    from_tip_id: source_tip_id.clone(),
                                    tip_id: terminal_tip_id.clone(),
                                    ended_at: record.ended_at,
                                },
                                // Upstream attaches `error` to the failed
                                // navigation_end; the fixed HarnessEvent union has
                                // no such field (module docs).
                                StructuralOutcome::Failed { .. } => HarnessEvent::NavigationEnd {
                                    lane: lane.name().to_owned(),
                                    run_id: operation_id.clone(),
                                    status: RunEndStatus::Failed,
                                    from_tip_id: source_tip_id.clone(),
                                    tip_id: terminal_tip_id.clone(),
                                    ended_at: record.ended_at,
                                },
                                _ => unreachable!("compaction outcomes rejected above"),
                            };
                            let lane_patch =
                                if matches!(outcome, StructuralOutcome::BranchSummary { .. }) {
                                    Some(LanePatch {
                                        tip_id: terminal_tip_id.clone(),
                                        configuration: None,
                                        inbox: None,
                                    })
                                } else {
                                    None
                                };
                            let record_for_materialize = record.clone();
                            Ok(OperationCommand::Finish {
                                writes: {
                                    writes.extend(cleanup);
                                    writes
                                },
                                record,
                                lane: lane_patch,
                                materialize: Box::new(move |_commit: &CommitResult| {
                                    StructuralPublication::procedure(ProcedureResult::Settled {
                                        outcome: record_for_materialize,
                                    })
                                }),
                                events: Some(Box::new(move |commit: &CommitResult| {
                                    let mut events =
                                        flatten_base_events(base_events, commit, None)?;
                                    events.push(navigation_end);
                                    Ok(events)
                                })),
                            })
                        }
                    }
                })
            },
            plan_context,
        )
        .await?;
    match published {
        ContinueOperationResult::CancelRequested => Ok(ProcedureResult::Continue),
        ContinueOperationResult::Result {
            value: StructuralPublication::Procedure(result),
        } => Ok(*result),
        ContinueOperationResult::Result {
            value: StructuralPublication::FinishPending { entry_ids },
        } => {
            let Some(task) = capability.phase.summary_task() else {
                return Err(invariant(
                    "Structural finish mediation requires a resumable finish boundary".to_string(),
                ));
            };
            match &task.boundary {
                ResultBoundary::ResumeCheckpoint { resume_after }
                    if matches!(resume_after.continuation, Continuation::MayFinish { .. }) =>
                {
                    let pending_events = vec![HarnessEvent::CompactionEnd {
                        lane: lane.name().to_owned(),
                        run_id: drive.operation_id().to_owned(),
                        reason: SummaryReason::Threshold,
                        status: CompactionEndStatus::Declined,
                        entry_id: None,
                        ended_at: crate::ai::now_ms(),
                    }];
                    finish_run_boundary(
                        lane,
                        drive,
                        capability,
                        &resume_after.continuation,
                        &entry_ids,
                        pending_events,
                        Arc::new(drive.gate().clone()),
                    )
                    .await
                }
                _ => Err(invariant(
                    "Structural finish mediation requires a resumable finish boundary".to_string(),
                )),
            }
        }
    }
}

/// Upstream `publishStructuralReady` (`structural.ts:579-605`): advance the
/// deciding task to `summary.ready` with its summary context.
async fn publish_structural_ready(
    lane: &Arc<Lane>,
    drive: &Drive,
    _deciding: &OperationState,
) -> anyhow::Result<ProcedureResult> {
    let result_entry_id = lane.session().id_generator().next(None);
    let planner_lane = Arc::clone(lane);
    let published = lane
        .continue_operation(
            move |state, current, _meta, _reader| {
                let lane = Arc::clone(&planner_lane);
                let result_entry_id = result_entry_id.clone();
                Box::pin(async move {
                    let OperationPhase::SummaryDeciding { task } = &current.phase else {
                        anyhow::bail!("Structural ready requires a summary.deciding operation");
                    };
                    let operation_state = OperationState {
                        scope: current.scope.clone(),
                        phase: OperationPhase::SummaryReady {
                            task: task.clone(),
                            summary_context: summary_context(
                                &lane,
                                result_entry_id,
                                state.configuration.clone(),
                            ),
                            next_attempt: 1,
                        },
                    };
                    Ok(OperationCommand::Commit {
                        writes: Vec::new(),
                        operation_state,
                        lane: None,
                        materialize: Box::new(|_commit: &CommitResult| {
                            StructuralPublication::procedure(ProcedureResult::Continue)
                        }),
                        events: None,
                    })
                })
            },
            drive.context().clone(),
        )
        .await?;
    Ok(match published {
        ContinueOperationResult::CancelRequested => ProcedureResult::Continue,
        ContinueOperationResult::Result {
            value: StructuralPublication::Procedure(result),
        } => *result,
        ContinueOperationResult::Result {
            value: StructuralPublication::FinishPending { .. },
        } => unreachable!("summary.ready commits never finish-pend"),
    })
}

/// Consume one durable structural preparation and decision hook.
/// Upstream `runStructuralDecision` (`structural.ts:607-672`).
pub async fn run_structural_decision(
    lane: &Arc<Lane>,
    drive: &Drive,
    deciding: &OperationState,
) -> anyhow::Result<ProcedureResult> {
    let preparation = read_structural_preparation(lane, drive, deciding).await?;
    let preparation = match preparation {
        ContinueOperationResult::CancelRequested => return Ok(ProcedureResult::Continue),
        ContinueOperationResult::Result { value } => value,
    };
    let Some(task) = deciding.phase.summary_task() else {
        anyhow::bail!("Structural decision requires a summary.deciding operation");
    };
    if let ResultBoundary::CommitNavigation { target_id, .. } = &task.boundary {
        let LivePreparation::BranchSummary(preparation) = preparation else {
            return Err(invariant(
                "Navigation task has invalid durable preparation".to_string(),
            ));
        };
        let hook = lane
            .hooks()
            .run_with_gate(
                HookInvocation {
                    lane: lane.name().to_owned(),
                    run_id: drive.operation_id().to_owned(),
                    event: HookEvent::BeforeNavigation(BeforeNavigationEvent {
                        target_id: target_id.clone(),
                        preparation,
                        custom_instructions: task.custom_instructions.clone(),
                    }),
                },
                Arc::new(drive.gate().clone()),
                drive.context().clone(),
            )
            .await?;
        let result = match hook {
            HookResult::BeforeNavigation(result) => result,
            other => {
                return Err(anyhow::anyhow!(
                    "hook registered for {} returned a {} result",
                    HookName::BeforeNavigation.as_str(),
                    other.hook_name().as_str()
                ));
            }
        };
        if result.as_ref().and_then(|result| result.decline) == Some(true) {
            return publish_structural_outcome(lane, drive, deciding, StructuralOutcome::Declined)
                .await;
        }
        if let Some(summary) = result.and_then(|result| result.summary) {
            return publish_structural_outcome(
                lane,
                drive,
                deciding,
                StructuralOutcome::BranchSummary {
                    result_entry_id: lane.session().id_generator().next(None),
                    result: summary,
                    from_hook: true,
                },
            )
            .await;
        }
        return publish_structural_ready(lane, drive, deciding).await;
    }

    let LivePreparation::Compaction(preparation) = preparation else {
        return Err(invariant(
            "Compaction task has invalid durable preparation".to_string(),
        ));
    };
    let reason = compaction_reason(task)?;
    let hook_reason = match reason {
        SummaryReason::Manual => crate::agent_core::harness::hooks::CompactionReason::Manual,
        SummaryReason::Threshold => crate::agent_core::harness::hooks::CompactionReason::Threshold,
        SummaryReason::Overflow => crate::agent_core::harness::hooks::CompactionReason::Overflow,
    };
    let hook = lane
        .hooks()
        .run_with_gate(
            HookInvocation {
                lane: lane.name().to_owned(),
                run_id: drive.operation_id().to_owned(),
                event: HookEvent::BeforeCompaction(BeforeCompactionEvent {
                    reason: hook_reason,
                    preparation,
                    custom_instructions: task.custom_instructions.clone(),
                }),
            },
            Arc::new(drive.gate().clone()),
            drive.context().clone(),
        )
        .await?;
    let result = match hook {
        HookResult::BeforeCompaction(result) => result,
        other => {
            return Err(anyhow::anyhow!(
                "hook registered for {} returned a {} result",
                HookName::BeforeCompaction.as_str(),
                other.hook_name().as_str()
            ));
        }
    };
    if result.as_ref().and_then(|result| result.decline) == Some(true) {
        return publish_structural_outcome(lane, drive, deciding, StructuralOutcome::Declined)
            .await;
    }
    if let Some(compaction) = result.and_then(|result| result.compaction) {
        return publish_structural_outcome(
            lane,
            drive,
            deciding,
            StructuralOutcome::Compaction {
                result_entry_id: lane.session().id_generator().next(None),
                result: compaction,
                from_hook: true,
            },
        )
        .await;
    }
    publish_structural_ready(lane, drive, deciding).await
}

/// Upstream `effectPendingFromReady` (`structural.ts:674-683`).
fn effect_pending_from_ready(ready: &OperationState) -> anyhow::Result<OperationState> {
    let OperationPhase::SummaryReady {
        task,
        summary_context,
        next_attempt,
    } = &ready.phase
    else {
        anyhow::bail!("Structural intent requires a summary.ready operation");
    };
    Ok(OperationState {
        scope: ready.scope.clone(),
        phase: OperationPhase::SummaryEffectPending {
            task: task.clone(),
            summary_context: summary_context.clone(),
            attempt: *next_attempt,
            request: None,
            usage_ids: Vec::new(),
        },
    })
}

/// Upstream `retryWaitFromEffect` (`structural.ts:685-695`).
fn retry_wait_from_effect(
    effect: &OperationState,
    error_message: String,
) -> anyhow::Result<OperationState> {
    let OperationPhase::SummaryEffectPending {
        task,
        summary_context,
        attempt,
        ..
    } = &effect.phase
    else {
        anyhow::bail!("Structural retry requires a summary.effect_pending operation");
    };
    let not_before = retry_not_before(
        &RetryDelayPolicy {
            base_delay_ms: summary_context.retry_policy.base_delay_ms as i64,
            max_agent_delay_ms: Some(summary_context.retry_policy.max_agent_delay_ms as i64),
        },
        i64::from(*attempt),
        crate::ai::now_ms(),
    );
    Ok(OperationState {
        scope: effect.scope.clone(),
        phase: OperationPhase::SummaryRetryWait {
            task: task.clone(),
            summary_context: summary_context.clone(),
            retry: RetryWait {
                next_attempt: attempt + 1,
                not_before,
                error_message,
            },
        },
    })
}

/// Upstream `readyFromRetryWait` (`structural.ts:697-705`).
fn ready_from_retry_wait(state: &OperationState) -> anyhow::Result<OperationState> {
    let OperationPhase::SummaryRetryWait {
        task,
        summary_context,
        retry: wait,
    } = &state.phase
    else {
        anyhow::bail!("Structural retry start requires a summary.retry_wait operation");
    };
    Ok(OperationState {
        scope: state.scope.clone(),
        phase: OperationPhase::SummaryReady {
            task: task.clone(),
            summary_context: summary_context.clone(),
            next_attempt: wait.next_attempt,
        },
    })
}

/// Upstream `publishAttemptIntent` (`structural.ts:707-725`).
async fn publish_attempt_intent(
    lane: &Arc<Lane>,
    drive: &Drive,
    _ready: &OperationState,
) -> anyhow::Result<ContinueOperationResult<OperationState>> {
    lane.continue_operation(
        move |_state, current, _meta, _reader| {
            Box::pin(async move {
                let effect_pending = effect_pending_from_ready(current)?;
                Ok(OperationCommand::Commit {
                    writes: Vec::new(),
                    operation_state: effect_pending.clone(),
                    lane: None,
                    materialize: Box::new(move |_commit: &CommitResult| effect_pending),
                    events: None,
                })
            })
        },
        drive.context().clone(),
    )
    .await
}

/// Upstream `publishNestedRequestIntent` (`structural.ts:727-747`).
async fn publish_nested_request_intent(
    lane: &Arc<Lane>,
    context: &Context,
    index: usize,
    usage_id: String,
) -> anyhow::Result<ContinueOperationResult<OperationState>> {
    lane.continue_operation(
        move |_state, current, _meta, _reader| {
            let usage_id = usage_id.clone();
            Box::pin(async move {
                let OperationPhase::SummaryEffectPending {
                    task,
                    summary_context,
                    attempt,
                    usage_ids,
                    ..
                } = &current.phase
                else {
                    anyhow::bail!(
                        "Nested request intent requires a summary.effect_pending operation"
                    );
                };
                let next = OperationState {
                    scope: current.scope.clone(),
                    phase: OperationPhase::SummaryEffectPending {
                        task: task.clone(),
                        summary_context: summary_context.clone(),
                        attempt: *attempt,
                        request: Some(DurableSummaryRequest {
                            index,
                            usage_id: usage_id.clone(),
                        }),
                        usage_ids: usage_ids.clone(),
                    },
                };
                Ok(OperationCommand::Commit {
                    writes: Vec::new(),
                    operation_state: next.clone(),
                    lane: None,
                    materialize: Box::new(move |_commit: &CommitResult| next),
                    events: None,
                })
            })
        },
        context.clone(),
    )
    .await
}

/// Upstream `publishNestedRequestOutcome` (`structural.ts:749-772`): record
/// the settled request's usage row and drop the durable request marker.
async fn publish_nested_request_outcome(
    lane: &Arc<Lane>,
    context: &Context,
    usage_id: String,
    response: &AssistantMessage,
) -> anyhow::Result<()> {
    let lane_name = lane.name().to_owned();
    let response = response.clone();
    lane.settle_operation(
        move |_state, current, _meta, _reader| {
            let lane_name = lane_name.clone();
            let usage_id = usage_id.clone();
            let response = response.clone();
            Box::pin(async move {
                let OperationPhase::SummaryEffectPending {
                    task,
                    summary_context,
                    attempt,
                    usage_ids,
                    ..
                } = &current.phase
                else {
                    anyhow::bail!(
                        "Nested request outcome requires a summary.effect_pending operation"
                    );
                };
                let row = NewUsageRow {
                    id: usage_id.clone(),
                    usage: response.usage,
                    entry_id: None,
                    adjustment: false,
                    details: None,
                };
                let mut next_usage_ids = usage_ids.clone();
                next_usage_ids.push(usage_id.clone());
                let next = OperationState {
                    scope: current.scope.clone(),
                    phase: OperationPhase::SummaryEffectPending {
                        task: task.clone(),
                        summary_context: summary_context.clone(),
                        attempt: *attempt,
                        request: None,
                        usage_ids: next_usage_ids,
                    },
                };
                Ok(OperationCommand::Commit {
                    writes: vec![insert_usage(row.clone())],
                    operation_state: next,
                    lane: None,
                    materialize: Box::new(|_commit: &CommitResult| ()),
                    events: Some(Box::new(move |commit: &CommitResult| {
                        Ok(vec![usage_event(&row, 0, commit, &lane_name)?])
                    })),
                })
            })
        },
        context.clone(),
    )
    .await
}

/// Upstream `requestStreamOptions` (`structural.ts:774-794`): project the
/// harness stream options onto one provider request. `telemetryContext` has no
/// ported field (module docs).
fn request_stream_options(
    mut options: SimpleStreamOptions,
    stream_options: &AgentHarnessStreamOptions,
    signal: Option<tokio_util::sync::CancellationToken>,
    on_payload: Option<Arc<crate::ai::types::request_callbacks::PayloadHook>>,
) -> SimpleStreamOptions {
    options.stream.transport = stream_options.transport;
    options.stream.timeout_ms = stream_options.timeout_ms;
    options.stream.max_retries = stream_options.max_retries;
    options.stream.max_retry_delay_ms = stream_options.max_retry_delay_ms;
    options.stream.headers = stream_options.headers.clone().map(|headers| {
        headers
            .into_iter()
            .map(|(key, value)| (key, Some(value)))
            .collect()
    });
    options.stream.metadata = stream_options.metadata.clone();
    options.stream.cache_retention = Some(CacheRetention::None);
    // Upstream hard-codes `deferred: false`. The port leaves the flag absent:
    // absent ≡ false for the deferred flag on every provider surface, and the
    // faux test provider (out of this slice's scope) reads `Some(false)` as a
    // deferred request (truthiness substitution in faux.rs).
    options.deferred = None;
    options.stream.signal = signal;
    options.stream.callbacks.on_payload = on_payload;
    options
}

/// Upstream the `StructuralCancelled` carrier: cancellation is observed by the
/// request closure and converted before the summary result is inspected
/// (module docs).
#[derive(Debug, Default)]
struct StructuralRequestShared {
    request_index: AtomicUsize,
    last_response: Mutex<Option<AssistantMessage>>,
    cancelled: AtomicBool,
    hard_error: Mutex<Option<anyhow::Error>>,
}

/// Upstream `StructuralAttempt` result (`structural.ts:796-806`); the
/// `retryable` flag is only consumed on the error branch (upstream computes it
/// alongside every result and reads it for errors only).
enum StructuralAttemptOutcome {
    Compaction {
        result: CompactResult,
    },
    BranchSummary {
        result: BranchSummaryResult,
    },
    Error {
        error: OperationError,
        retryable: bool,
    },
    CancelRequested,
}

/// An abort refusal recognized at a hook/gate boundary (upstream
/// `error instanceof AbortRequested`); mirrors the tools.rs helper.
enum AbortRefusal {
    Requested(AbortRequested),
    CancelledContext,
}

fn abort_refusal(error: &anyhow::Error) -> Option<AbortRefusal> {
    if let Some(rejection) = error.downcast_ref::<GateRejection>() {
        if let GateRejection::AbortRequested(abort) = rejection {
            return Some(AbortRefusal::Requested(AbortRequested {
                cancellation: abort.cancellation.clone(),
            }));
        }
        return None;
    }
    if error.to_string() == "the operation was aborted" {
        return Some(AbortRefusal::CancelledContext);
    }
    None
}

/// The synthetic aborted message steering the summary request into its error
/// branch after an upstream `StructuralCancelled` throw (module docs).
fn aborted_summary_message() -> AssistantMessage {
    serde_json::from_value(serde_json::json!({
        "role": "assistant",
        "content": [],
        "api": "unknown",
        "provider": "structural",
        "model": "structural",
        "usage": {
            "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0,
            "totalTokens": 0,
            "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0}
        },
        "stopReason": "aborted",
        "errorMessage": "the operation was aborted",
        "timestamp": 0
    }))
    .expect("aborted message literal parses")
}

/// Upstream `performStructuralAttempt` (`structural.ts:796-923`): run one
/// generation attempt through the caller-owned request boundary.
#[allow(clippy::too_many_lines)]
async fn perform_structural_attempt(
    lane: &Arc<Lane>,
    drive: &Drive,
    effect: &OperationState,
    model: Model,
    preparation: LivePreparation,
) -> anyhow::Result<StructuralAttemptOutcome> {
    let OperationPhase::SummaryEffectPending {
        task,
        summary_context,
        attempt,
        ..
    } = &effect.phase
    else {
        anyhow::bail!("Structural attempt requires a summary.effect_pending operation");
    };
    let task = task.clone();
    let summary_context = summary_context.clone();
    let attempt = *attempt;
    let kind = summary_kind(&task);
    let step = if kind == "compaction" {
        RequestStep::Compaction
    } else {
        RequestStep::BranchSummary
    };
    let mut base_stream_options = summary_context.stream_options.clone();
    base_stream_options.deferred = Some(DeferredFlag::Bool(false));

    let shared = Arc::new(StructuralRequestShared {
        request_index: AtomicUsize::new(0),
        last_response: Mutex::new(None),
        cancelled: AtomicBool::new(false),
        hard_error: Mutex::new(None),
    });

    let request: SummaryRequest = {
        let lane = Arc::clone(lane);
        let gate = drive.gate().clone();
        let operation_id = drive.operation_id().to_owned();
        let model = model.clone();
        let base_stream_options = base_stream_options.clone();
        let shared = Arc::clone(&shared);
        Arc::new(
            move |ai_context: AiContext, options: SimpleStreamOptions, request_context: Context| {
                let lane = Arc::clone(&lane);
                let gate = gate.clone();
                let operation_id = operation_id.clone();
                let model = model.clone();
                let base_stream_options = base_stream_options.clone();
                let shared = Arc::clone(&shared);
                Box::pin(async move {
                    // A prior request already failed or was cancelled: short-circuit
                    // like the upstream throw would have.
                    if shared.cancelled.load(Ordering::SeqCst)
                        || shared
                            .hard_error
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .is_some()
                    {
                        return aborted_summary_message();
                    }

                    // before_request (`structural.ts:812-830`).
                    let before_request = lane
                        .hooks()
                        .run_with_gate(
                            HookInvocation {
                                lane: lane.name().to_owned(),
                                run_id: operation_id.clone(),
                                event: HookEvent::BeforeRequest(BeforeRequestEvent {
                                    model: model.clone(),
                                    step,
                                    attempt,
                                    stream_options: base_stream_options.clone(),
                                }),
                            },
                            Arc::new(gate.clone()),
                            request_context.clone(),
                        )
                        .await;
                    let stream_options: AgentHarnessStreamOptions = match before_request {
                        Ok(HookResult::BeforeRequest(patch)) => {
                            let mut merged = match &patch {
                                Some(result) => apply_stream_options_patch(
                                    base_stream_options.clone(),
                                    &result.stream_options,
                                ),
                                None => base_stream_options.clone(),
                            };
                            merged.deferred = Some(DeferredFlag::Bool(false));
                            merged
                        }
                        Ok(other) => {
                            *shared
                                .hard_error
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner) = Some(anyhow::anyhow!(
                                "hook registered for {} returned a {} result",
                                HookName::BeforeRequest.as_str(),
                                other.hook_name().as_str()
                            ));
                            return aborted_summary_message();
                        }
                        Err(error) => match abort_refusal(&error) {
                            Some(AbortRefusal::Requested(abort)) => {
                                abort.cancellation.cancelled().await;
                                shared.cancelled.store(true, Ordering::SeqCst);
                                return aborted_summary_message();
                            }
                            Some(AbortRefusal::CancelledContext) => {
                                shared.cancelled.store(true, Ordering::SeqCst);
                                return aborted_summary_message();
                            }
                            None => {
                                *shared
                                    .hard_error
                                    .lock()
                                    .unwrap_or_else(PoisonError::into_inner) = Some(error);
                                return aborted_summary_message();
                            }
                        },
                    };

                    // Durable nested-request intent (`structural.ts:837-840`).
                    let usage_id = lane.session().id_generator().next(None);
                    let index = shared.request_index.fetch_add(1, Ordering::SeqCst);
                    let intent = publish_nested_request_intent(
                        &lane,
                        &request_context,
                        index,
                        usage_id.clone(),
                    )
                    .await;
                    let intent = match intent {
                        Ok(intent) => intent,
                        Err(error) => {
                            *shared
                                .hard_error
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner) = Some(error);
                            return aborted_summary_message();
                        }
                    };
                    let ContinueOperationResult::Result { .. } = intent else {
                        shared.cancelled.store(true, Ordering::SeqCst);
                        return aborted_summary_message();
                    };

                    // Provider call under the drive gate (`structural.ts:841-863`).
                    let admitted_context =
                        with_abort_signal(gate.signal(), request_context.clone());
                    let models = lane.models();
                    let request_options = request_stream_options(
                        options,
                        &stream_options,
                        admitted_context.abort_signal(),
                        Some(Arc::new({
                            let lane = Arc::clone(&lane);
                            let gate = gate.clone();
                            let operation_id = operation_id.clone();
                            let admitted_context = admitted_context.clone();
                            move |payload: serde_json::Value, request_model: Model| {
                                let lane = Arc::clone(&lane);
                                let gate = gate.clone();
                                let operation_id = operation_id.clone();
                                let admitted_context = admitted_context.clone();
                                Box::pin(async move {
                                    let hook = lane
                                        .hooks()
                                        .run_with_gate(
                                            HookInvocation {
                                                lane: lane.name().to_owned(),
                                                run_id: operation_id,
                                                event: HookEvent::BeforePayload(
                                                    BeforePayloadEvent {
                                                        model: request_model,
                                                        payload,
                                                    },
                                                ),
                                            },
                                            Arc::new(gate),
                                            admitted_context,
                                        )
                                        .await?;
                                    Ok(match hook {
                                        HookResult::BeforePayload(Some(result)) => {
                                            Some(result.payload)
                                        }
                                        HookResult::BeforePayload(None) => None,
                                        other => {
                                            return Err(anyhow::anyhow!(
                                                "hook registered for {} returned a {} result",
                                                HookName::BeforePayload.as_str(),
                                                other.hook_name().as_str()
                                            ));
                                        }
                                    })
                                })
                            }
                        })),
                    );
                    let admitted = gate.admit(|| {
                        models.complete_simple(
                            &model,
                            &ai_context,
                            Some(ModelsSimpleStreamOptions {
                                simple: request_options,
                                transform_headers: None,
                            }),
                        )
                    });
                    let response: AssistantMessage = match admitted {
                        Ok(response) => response.await,
                        Err(rejection) => match rejection {
                            GateRejection::AbortRequested(abort) => {
                                abort.cancellation.cancelled().await;
                                shared.cancelled.store(true, Ordering::SeqCst);
                                return aborted_summary_message();
                            }
                            other => {
                                *shared
                                    .hard_error
                                    .lock()
                                    .unwrap_or_else(PoisonError::into_inner) =
                                    Some(anyhow::Error::new(other));
                                return aborted_summary_message();
                            }
                        },
                    };

                    // Upstream records `lastResponse` before settling the outcome
                    // so the caller can judge retryability.
                    *shared
                        .last_response
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner) = Some(response.clone());

                    // Settle the nested request outcome (`structural.ts:864-866`).
                    if let Err(error) =
                        publish_nested_request_outcome(&lane, &request_context, usage_id, &response)
                            .await
                    {
                        *shared
                            .hard_error
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner) = Some(error);
                        return aborted_summary_message();
                    }
                    response
                })
            },
        )
    };

    let result = if kind == "compaction" {
        let LivePreparation::Compaction(preparation) = preparation else {
            return Err(invariant(
                "Compaction summary has invalid durable preparation".to_string(),
            ));
        };
        let outcome = compact_with_request(
            preparation,
            &CompactGenerationOptions {
                model: model.clone(),
                custom_instructions: task.custom_instructions.clone(),
                thinking_level: Some(summary_context.configuration.thinking_level),
            },
            &request,
            drive.context().clone(),
        )
        .await;
        match outcome {
            Ok(result) => StructuralAttemptOutcome::Compaction { result },
            Err(error) => StructuralAttemptOutcome::Error {
                error: operation_error(error_code_str(error.code), error.message),
                retryable: last_response_is_retryable(&shared),
            },
        }
    } else {
        let LivePreparation::BranchSummary(preparation) = preparation else {
            return Err(invariant(
                "Branch summary has invalid durable preparation".to_string(),
            ));
        };
        let outcome = generate_branch_summary_with_request(
            preparation,
            &PreparedBranchSummaryOptions {
                custom_instructions: task.custom_instructions.clone(),
                replace_instructions: false,
            },
            &request,
            drive.context().clone(),
        )
        .await;
        match outcome {
            Ok(result) => StructuralAttemptOutcome::BranchSummary { result },
            Err(error) => StructuralAttemptOutcome::Error {
                error: operation_error(branch_error_code_str(error.code), error.message),
                retryable: last_response_is_retryable(&shared),
            },
        }
    };

    if let Some(error) = shared
        .hard_error
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
    {
        return Err(error);
    }
    if shared.cancelled.load(Ordering::SeqCst) {
        return Ok(StructuralAttemptOutcome::CancelRequested);
    }
    Ok(result)
}

fn last_response_is_retryable(shared: &StructuralRequestShared) -> bool {
    let last = shared
        .last_response
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    // Upstream never reaches `isRetryableAssistantError` without a response in
    // these branches: `lastResponse !== undefined && ...`.
    match last.as_ref() {
        Some(message) => is_retryable_assistant_error(message),
        None => false,
    }
}

fn error_code_str(code: crate::agent_core::harness::types::CompactionErrorCode) -> &'static str {
    match code {
        crate::agent_core::harness::types::CompactionErrorCode::Aborted => "aborted",
        crate::agent_core::harness::types::CompactionErrorCode::SummarizationFailed => {
            "summarization_failed"
        }
    }
}

fn branch_error_code_str(
    code: crate::agent_core::harness::types::BranchSummaryErrorCode,
) -> &'static str {
    match code {
        crate::agent_core::harness::types::BranchSummaryErrorCode::Aborted => "aborted",
        crate::agent_core::harness::types::BranchSummaryErrorCode::SummarizationFailed => {
            "summarization_failed"
        }
    }
}

/// Upstream `readAttemptPreparation` (`structural.ts:925-951`).
async fn read_attempt_preparation(
    lane: &Arc<Lane>,
    drive: &Drive,
    _ready: &OperationState,
) -> anyhow::Result<ContinueOperationResult<LivePreparation>> {
    let operation_id = drive.operation_id().to_owned();
    let plan_context = drive.context().clone();
    let context = plan_context.clone();
    lane.continue_operation(
        move |_state, current, _meta, reader| {
            let operation_id = operation_id.clone();
            let context = context.clone();
            Box::pin(async move {
                let Some(task) = current.phase.summary_task() else {
                    anyhow::bail!("Structural attempt preparation requires a summary operation");
                };
                let expected = summary_kind(task);
                let stored = reader
                    .get_value(
                        &operation_preparation(&operation_id, &task.task_id),
                        context.clone(),
                    )
                    .await?;
                let kind_of = stored
                    .as_ref()
                    .and_then(|stored| stored.value.get("kind"))
                    .and_then(|kind| kind.as_str());
                if stored.is_none() || kind_of != Some(expected) {
                    return Err(invariant(format!(
                        "Structural task {} has invalid durable preparation",
                        task.task_id
                    )));
                }
                let durable =
                    parse_durable_structural_preparation(stored.expect("checked above").value)?;
                let preparation = match durable {
                    DurableStructuralPreparation::Compaction(value) => {
                        LivePreparation::Compaction(live_compaction_preparation(value))
                    }
                    DurableStructuralPreparation::BranchSummary(value) => {
                        LivePreparation::BranchSummary(live_branch_preparation(value))
                    }
                };
                Ok(OperationCommand::Return {
                    result: preparation,
                })
            })
        },
        plan_context,
    )
    .await
}

/// Upstream `publishAttemptResult` (`structural.ts:953-1007`).
#[allow(clippy::too_many_lines)]
async fn publish_attempt_result(
    lane: &Arc<Lane>,
    drive: &Drive,
    effect: &OperationState,
    result: StructuralAttemptOutcome,
) -> anyhow::Result<ProcedureResult> {
    match result {
        StructuralAttemptOutcome::CancelRequested => Ok(ProcedureResult::Continue),
        StructuralAttemptOutcome::Compaction { result } => {
            publish_structural_outcome(
                lane,
                drive,
                effect,
                StructuralOutcome::Compaction {
                    result_entry_id: summary_result_entry_id(effect),
                    result,
                    from_hook: false,
                },
            )
            .await
        }
        StructuralAttemptOutcome::BranchSummary { result } => {
            publish_structural_outcome(
                lane,
                drive,
                effect,
                StructuralOutcome::BranchSummary {
                    result_entry_id: summary_result_entry_id(effect),
                    result,
                    from_hook: false,
                },
            )
            .await
        }
        StructuralAttemptOutcome::Error { error, retryable } => {
            if error.code == "aborted" {
                if let Some(operation) = &lane.state().operation {
                    if matches!(operation.state.scope.control, Control::Running) {
                        return Err(invariant(
                            "Structural provider response is aborted while durable control is running"
                                .to_string(),
                        ));
                    }
                }
            }
            let OperationPhase::SummaryEffectPending {
                attempt,
                summary_context,
                ..
            } = &effect.phase
            else {
                anyhow::bail!(
                    "Structural attempt result requires a summary.effect_pending operation"
                );
            };
            let max_attempts = summary_context.retry_policy.max_attempts;
            if retryable && *attempt < max_attempts {
                let retry_wait = retry_wait_from_effect(effect, error.message.clone())?;
                let OperationPhase::SummaryRetryWait { retry, .. } = &retry_wait.phase else {
                    unreachable!("retry_wait_from_effect builds a retry-wait phase");
                };
                let next_attempt = retry.next_attempt;
                let not_before = retry.not_before;
                let delay_ms = retry_delay_ms(
                    &crate::ai::retry::RetryPolicy {
                        enabled: true,
                        max_retries: u32::MAX,
                        base_delay_ms: summary_context.retry_policy.base_delay_ms,
                        max_agent_delay_ms: Some(summary_context.retry_policy.max_agent_delay_ms),
                    },
                    *attempt,
                ) as i64;
                let lane_name = lane.name().to_owned();
                let run_id = drive.operation_id().to_owned();
                let step = task_id_of(effect);
                let error_message = error.message.clone();
                let published = lane
                    .continue_operation(
                        move |_state, _current, _meta, _reader| {
                            let retry_wait = retry_wait.clone();
                            let lane_name = lane_name.clone();
                            let run_id = run_id.clone();
                            let step = step.clone();
                            let error_message = error_message.clone();
                            Box::pin(async move {
                                Ok(OperationCommand::Commit {
                                    writes: Vec::new(),
                                    operation_state: retry_wait,
                                    lane: None,
                                    materialize: Box::new(|_commit: &CommitResult| {
                                        StructuralPublication::procedure(ProcedureResult::Continue)
                                    }),
                                    events: Some(Box::new(move |_commit: &CommitResult| {
                                        Ok(vec![HarnessEvent::RetryScheduled {
                                            lane: lane_name,
                                            run_id,
                                            step,
                                            attempt: next_attempt,
                                            max_attempts,
                                            delay_ms,
                                            not_before,
                                            error_message,
                                        }])
                                    })),
                                })
                            })
                        },
                        drive.context().clone(),
                    )
                    .await?;
                return Ok(match published {
                    ContinueOperationResult::CancelRequested => ProcedureResult::Continue,
                    ContinueOperationResult::Result {
                        value: StructuralPublication::Procedure(result),
                    } => *result,
                    ContinueOperationResult::Result {
                        value: StructuralPublication::FinishPending { .. },
                    } => unreachable!("retry waits never finish-pend"),
                });
            }
            publish_structural_outcome(lane, drive, effect, StructuralOutcome::Failed { error })
                .await
        }
    }
}

fn summary_result_entry_id(effect: &OperationState) -> String {
    match &effect.phase {
        OperationPhase::SummaryEffectPending {
            summary_context, ..
        } => summary_context.result_entry_id.clone(),
        OperationPhase::SummaryReady {
            summary_context, ..
        } => summary_context.result_entry_id.clone(),
        _ => String::new(),
    }
}

fn task_id_of(state: &OperationState) -> String {
    state
        .phase
        .summary_task()
        .map(|task| task.task_id.clone())
        .unwrap_or_default()
}

/// Execute one ready structural generation attempt.
/// Upstream `runStructuralGeneration` (`structural.ts:1009-1029`).
pub async fn run_structural_generation(
    lane: &Arc<Lane>,
    drive: &Drive,
    ready: &OperationState,
) -> anyhow::Result<ProcedureResult> {
    let preparation = read_attempt_preparation(lane, drive, ready).await?;
    let preparation = match preparation {
        ContinueOperationResult::CancelRequested => return Ok(ProcedureResult::Continue),
        ContinueOperationResult::Result { value } => value,
    };
    let OperationPhase::SummaryReady {
        summary_context, ..
    } = &ready.phase
    else {
        anyhow::bail!("Structural generation requires a summary.ready operation");
    };
    let identity = summary_context.configuration.model.clone();
    let Some(model) = lane
        .models()
        .get_model(&identity.provider, &identity.model_id)
    else {
        return publish_structural_outcome(
            lane,
            drive,
            ready,
            StructuralOutcome::Failed {
                error: OperationError {
                    code: "model_unavailable".to_string(),
                    message: "The configured model is unavailable in this process".to_string(),
                    details: Some(serde_json::to_value(&identity)?),
                },
            },
        )
        .await;
    };
    let intent = publish_attempt_intent(lane, drive, ready).await?;
    let intent = match intent {
        ContinueOperationResult::CancelRequested => return Ok(ProcedureResult::Continue),
        ContinueOperationResult::Result { value } => value,
    };
    let result = perform_structural_attempt(lane, drive, &intent, model, preparation).await?;
    publish_attempt_result(lane, drive, &intent, result).await
}

/// Consume one structural retry wait without starting a provider effect.
/// Upstream `runStructuralRetryWait` (`structural.ts:1031-1074`).
pub async fn run_structural_retry_wait(
    lane: &Arc<Lane>,
    drive: &Drive,
    retry: &OperationState,
) -> anyhow::Result<ProcedureResult> {
    let OperationPhase::SummaryRetryWait { retry: wait, .. } = &retry.phase else {
        anyhow::bail!("Structural retry wait requires a summary.retry_wait operation");
    };
    let not_before = wait.not_before;
    if crate::ai::now_ms() < not_before {
        if !drive.wait_for_retry() {
            return Ok(ProcedureResult::Waiting {
                outcome: DriveOutcome::Waiting {
                    operation_id: drive.operation_id().to_string(),
                    reason: WaitingReason::Retry { not_before },
                },
            });
        }
        // The generation.rs run_retry_wait admit/error dance: preserve the
        // gate's typed abort/close refusal when cancellation wins.
        let reason: Arc<dyn std::error::Error + Send + Sync> =
            Arc::new(std::io::Error::other("retry wait aborted"));
        let waiting = drive
            .gate()
            .admit(|| wait_until(not_before, drive.gate().signal(), reason))?;
        if let Err(error) = waiting.await {
            drive.gate().admit(|| ())?;
            return Err(error);
        }
    }
    let lane_name = lane.name().to_owned();
    let run_id = drive.operation_id().to_owned();
    let published = lane
        .continue_operation(
            move |_state, current, _meta, _reader| {
                let lane_name = lane_name.clone();
                let run_id = run_id.clone();
                Box::pin(async move {
                    let ready = ready_from_retry_wait(current)?;
                    let OperationPhase::SummaryReady {
                        task, next_attempt, ..
                    } = &ready.phase
                    else {
                        unreachable!("ready_from_retry_wait builds a ready phase");
                    };
                    let step = task.task_id.clone();
                    let attempt = *next_attempt;
                    Ok(OperationCommand::Commit {
                        writes: Vec::new(),
                        operation_state: ready,
                        lane: None,
                        materialize: Box::new(|_commit: &CommitResult| {
                            StructuralPublication::procedure(ProcedureResult::Continue)
                        }),
                        events: Some(Box::new(move |_commit: &CommitResult| {
                            Ok(vec![HarnessEvent::RetryStart {
                                lane: lane_name,
                                run_id,
                                step,
                                attempt,
                            }])
                        })),
                    })
                })
            },
            drive.context().clone(),
        )
        .await?;
    Ok(match published {
        ContinueOperationResult::CancelRequested => ProcedureResult::Continue,
        ContinueOperationResult::Result {
            value: StructuralPublication::Procedure(result),
        } => *result,
        ContinueOperationResult::Result {
            value: StructuralPublication::FinishPending { .. },
        } => unreachable!("retry starts never finish-pend"),
    })
}

/// Convert an orphaned structural attempt into a fresh numbered attempt or
/// terminal failure. Upstream `recoverStructuralGeneration`
/// (`structural.ts:1076-1115`).
pub async fn recover_structural_generation(
    lane: &Arc<Lane>,
    drive: &Drive,
    effect: &OperationState,
) -> anyhow::Result<ProcedureResult> {
    let error = operation_error(
        "structural_interrupted",
        "Structural summary attempt was interrupted and its external outcome is unknown",
    );
    let OperationPhase::SummaryEffectPending {
        attempt,
        summary_context,
        ..
    } = &effect.phase
    else {
        anyhow::bail!("Structural recovery requires a summary.effect_pending operation");
    };
    if *attempt >= summary_context.retry_policy.max_attempts {
        return publish_structural_outcome(
            lane,
            drive,
            effect,
            StructuralOutcome::Failed { error },
        )
        .await;
    }
    let retry_wait = retry_wait_from_effect(effect, error.message.clone())?;
    let OperationPhase::SummaryRetryWait { retry, .. } = &retry_wait.phase else {
        unreachable!("retry_wait_from_effect builds a retry-wait phase");
    };
    let next_attempt = retry.next_attempt;
    let not_before = retry.not_before;
    let delay_ms = retry_delay_ms(
        &crate::ai::retry::RetryPolicy {
            enabled: true,
            max_retries: u32::MAX,
            base_delay_ms: summary_context.retry_policy.base_delay_ms,
            max_agent_delay_ms: Some(summary_context.retry_policy.max_agent_delay_ms),
        },
        *attempt,
    ) as i64;
    let lane_name = lane.name().to_owned();
    let run_id = drive.operation_id().to_owned();
    let step = task_id_of(effect);
    let error_message = error.message.clone();
    let max_attempts = summary_context.retry_policy.max_attempts;
    let published = lane
        .continue_operation(
            move |_state, _current, _meta, _reader| {
                let retry_wait = retry_wait.clone();
                let lane_name = lane_name.clone();
                let run_id = run_id.clone();
                let step = step.clone();
                let error_message = error_message.clone();
                Box::pin(async move {
                    Ok(OperationCommand::Commit {
                        writes: Vec::new(),
                        operation_state: retry_wait,
                        lane: None,
                        materialize: Box::new(|_commit: &CommitResult| {
                            StructuralPublication::procedure(ProcedureResult::Continue)
                        }),
                        // Upstream adds `recovery: true`; the fixed HarnessEvent
                        // union has no such field (module docs).
                        events: Some(Box::new(move |_commit: &CommitResult| {
                            Ok(vec![HarnessEvent::RetryScheduled {
                                lane: lane_name,
                                run_id,
                                step,
                                attempt: next_attempt,
                                max_attempts,
                                delay_ms,
                                not_before,
                                error_message,
                            }])
                        })),
                    })
                })
            },
            drive.context().clone(),
        )
        .await?;
    Ok(match published {
        ContinueOperationResult::CancelRequested => ProcedureResult::Continue,
        ContinueOperationResult::Result {
            value: StructuralPublication::Procedure(result),
        } => *result,
        ContinueOperationResult::Result {
            value: StructuralPublication::FinishPending { .. },
        } => unreachable!("recoveries never finish-pend"),
    })
}

/// Prepare threshold compaction only when no newer compaction already guards
/// this trigger. Upstream `prepareCompactionThreshold`
/// (`structural.ts:1117-1152`).
pub async fn prepare_compaction_threshold(
    lane: &std::sync::Arc<Lane>,
    drive: &Drive,
    checkpoint: &OperationState,
) -> anyhow::Result<ContinueOperationResult<Option<StructuralPreparation>>> {
    let settings = checkpoint.scope.settings.compaction;
    let configuration = lane
        .read_lane(
            |state, _reader| Box::pin(async move { Ok(state.configuration.clone()) }),
            drive.context().clone(),
        )
        .await?;
    let model = lane
        .models()
        .get_model(&configuration.model.provider, &configuration.model.model_id);
    if !settings.enabled || model.is_none() {
        return Ok(ContinueOperationResult::Result { value: None });
    }
    let model = model.expect("checked above");
    let path = match read_bounded_entries(lane, drive, checkpoint).await? {
        ContinueOperationResult::CancelRequested => {
            return Ok(ContinueOperationResult::CancelRequested);
        }
        ContinueOperationResult::Result { value } => value,
    };
    let trigger_entry_id = match &checkpoint.phase {
        crate::agent_core::harness::runtime::durable::OperationPhase::Checkpoint { checkpoint } => {
            checkpoint.trigger_entry_id.clone()
        }
        _ => anyhow::bail!("Threshold compaction requires a checkpoint operation"),
    };
    // Upstream order: the durable guard is checked BEFORE the missing-trigger
    // invariant (a -1 triggerIndex loses to any newer compaction).
    let trigger_index = path.iter().position(|entry| entry.id() == trigger_entry_id);
    let newest_compaction_index = path.iter().rposition(|entry| {
        matches!(
            entry,
            crate::agent_core::harness::session::Entry::Compaction { .. }
        )
    });
    if newest_compaction_index
        .is_some_and(|newest| trigger_index.is_none_or(|trigger| newest >= trigger))
    {
        return Ok(ContinueOperationResult::Result { value: None });
    }
    if trigger_index.is_none() {
        anyhow::bail!(
            "Checkpoint trigger {} is missing from its Branch",
            trigger_entry_id
        );
    }
    let prepared = prepare_compaction(&path, settings)?;
    match prepared {
        None => Ok(ContinueOperationResult::Result { value: None }),
        Some(preparation) => {
            if !should_compact(preparation.tokens_before, model.context_window, settings) {
                return Ok(ContinueOperationResult::Result { value: None });
            }
            Ok(ContinueOperationResult::Result {
                value: Some(StructuralPreparation {
                    task_id: lane.session().id_generator().next(None),
                    preparation: durable_compaction_preparation(&preparation),
                }),
            })
        }
    }
}

/// Prepare one overflow compaction before the response settlement transaction.
/// Upstream `prepareOverflowCompaction` (`structural.ts:1154-1170`).
pub async fn prepare_overflow_compaction(
    lane: &std::sync::Arc<Lane>,
    drive: &Drive,
    generation: &OperationState,
) -> anyhow::Result<Option<StructuralPreparation>> {
    let crate::agent_core::harness::runtime::durable::OperationPhase::AssistantEffectPending {
        generation_context,
        ..
    } = &generation.phase
    else {
        anyhow::bail!("Overflow recovery requires an assistant effect-pending operation");
    };
    if generation_context.overflow_recovery_used {
        return Ok(None);
    }
    let path = match read_bounded_entries(lane, drive, generation).await? {
        ContinueOperationResult::CancelRequested => return Ok(None),
        ContinueOperationResult::Result { value } => value,
    };
    let prepared = prepare_compaction(&path, generation.scope.settings.compaction)?;
    let Some(preparation) = prepared else {
        return Ok(None);
    };
    Ok(Some(StructuralPreparation {
        task_id: lane.session().id_generator().next(None),
        preparation: durable_compaction_preparation(&preparation),
    }))
}

/// Atomically move an unsummarized navigation and finish its operation.
/// Upstream `commitNavigation` (`structural.ts:1172-1222`).
#[allow(clippy::too_many_lines)]
pub async fn commit_navigation(
    lane: &Arc<Lane>,
    drive: &Drive,
    _navigation: &OperationState,
) -> anyhow::Result<ProcedureResult> {
    let operation_id = drive.operation_id().to_owned();
    let lane_name = lane.name().to_owned();
    let plan_context = drive.context().clone();
    let context = plan_context.clone();
    let result = lane
        .continue_operation(
            move |_state, current, meta, reader| {
                let operation_id = operation_id.clone();
                let lane_name = lane_name.clone();
                let context = context.clone();
                let source_tip_id = meta.source_tip_id.clone();
                Box::pin(async move {
                    let OperationPhase::NavigationReadyToCommit {
                        target_id: current_target,
                        label: current_label,
                    } = &current.phase
                    else {
                        anyhow::bail!(
                            "Navigation commit requires a navigation.ready_to_commit operation"
                        );
                    };
                    let current_target = current_target.clone();
                    let current_label = current_label.clone();
                    if let Some(target) = &current_target {
                        let entries = reader
                            .get_entries(std::slice::from_ref(target), context.clone())
                            .await?;
                        if !entries.contains_key(target) {
                            return Err(invariant(format!(
                                "Navigation target {} is missing",
                                target
                            )));
                        }
                    }
                    if current_target == source_tip_id {
                        return Err(invariant(
                            "Navigation target must differ from its source tip".to_string(),
                        ));
                    }
                    if current_target.is_none() && current_label.is_some() {
                        return Err(invariant("Root navigation cannot set a label".to_string()));
                    }
                    let mut writes: Vec<Write> = vec![set_value(
                        &branch_tip(&lane_name),
                        current_target
                            .clone()
                            .map(serde_json::Value::String)
                            .unwrap_or(serde_json::Value::Null),
                    )];
                    if let (Some(label), Some(target)) = (&current_label, &current_target) {
                        writes.push(set_value(
                            &entry_label(target),
                            serde_json::Value::String(label.clone()),
                        ));
                    }
                    let cleanup =
                        operation_cleanup_writes(reader, &operation_id, current, context.clone())
                            .await?;
                    writes.extend(cleanup);
                    let record = operation_result_record(
                        meta,
                        TerminalStatus::Completed,
                        current_target.clone(),
                        None,
                    )?;
                    let ended_at = record.ended_at;
                    let record_for_materialize = record.clone();
                    Ok(OperationCommand::Finish {
                        writes,
                        record,
                        // Upstream sets the lane tip to current.targetId; a
                        // null root target cannot be expressed in LanePatch
                        // (module docs).
                        lane: Some(LanePatch {
                            tip_id: current_target.clone(),
                            configuration: None,
                            inbox: None,
                        }),
                        materialize: Box::new(move |_commit: &CommitResult| {
                            StructuralPublication::procedure(ProcedureResult::Settled {
                                outcome: record_for_materialize,
                            })
                        }),
                        events: Some(Box::new(move |_commit: &CommitResult| {
                            Ok(vec![HarnessEvent::NavigationEnd {
                                lane: lane_name,
                                run_id: operation_id,
                                status: RunEndStatus::Completed,
                                from_tip_id: source_tip_id,
                                tip_id: current_target.clone(),
                                ended_at,
                            }])
                        })),
                    })
                })
            },
            plan_context,
        )
        .await?;
    Ok(match result {
        ContinueOperationResult::CancelRequested => ProcedureResult::Continue,
        ContinueOperationResult::Result {
            value: StructuralPublication::Procedure(result),
        } => *result,
        ContinueOperationResult::Result {
            value: StructuralPublication::FinishPending { .. },
        } => unreachable!("navigation commits never finish-pend"),
    })
}

#[cfg(test)]
#[path = "structural_tests.rs"]
mod tests;
