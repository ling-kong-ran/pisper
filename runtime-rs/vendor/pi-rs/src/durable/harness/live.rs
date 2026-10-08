//! Port of `src/harness/live.ts`: the built-in `pi.live` conversation state —
//! run control and the presentation of the current generation attempt and
//! tool round.
//!
//! Divergences (structural, disclosed): upstream mutates the typed
//! `Draft<LiveState>` document value; the port mutates the same JSON through
//! [`DocumentDraft`] path writes, so each upstream field assignment maps to a
//! set/delete at the same key (Chord diffs string leaves itself), and
//! `settleSchedulerOutcome` lives here with the scheduler slice
//! (it consumes `convertPartial` from `generation.rs` and the scheduler's
//! outcome shape).

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Map, Value};

use super::super::documents::{define_doc, DefinitionScope, DocToken};
use super::super::errors::PlainError;
use super::super::ids::{EntryId, SubmissionId, TaskId};
use super::super::session::transaction::{DocumentDraft, Transaction};
use super::super::types::{
    DocumentFork, DocumentHistory, JsonObject, SubmissionSettlement, TaskOutcome, TaskRecord,
};

/// Presentation of one tool call of the current round (`live.ts`
/// `ToolSlot`). Wire order `{callId, name, taskId?, status, output?,
/// droppedBytes?, droppedLines?, details?, diagnostics?, entry?}`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ToolSlot {
    pub call_id: String,
    pub name: String,
    /// Absent for a call not started yet (sequential round) and for a call
    /// its request did not offer.
    pub task_id: Option<TaskId>,
    pub status: SlotStatus,
    /// Retained running output and what the bounds dropped.
    pub output: Option<String>,
    pub dropped_bytes: Option<f64>,
    pub dropped_lines: Option<f64>,
    /// Last `details()` value.
    pub details: Option<Value>,
    /// Diagnostics recorded through `api.diagnostic()`.
    pub diagnostics: Option<Vec<Value>>,
    /// Result entry once done; absent when the tool task faulted or was
    /// orphaned.
    pub entry: Option<EntryId>,
}

/// `ToolSlot.status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SlotStatus {
    #[default]
    Pending,
    Running,
    Done,
}

impl SlotStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            SlotStatus::Pending => "pending",
            SlotStatus::Running => "running",
            SlotStatus::Done => "done",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "pending" => Some(SlotStatus::Pending),
            "running" => Some(SlotStatus::Running),
            "done" => Some(SlotStatus::Done),
            _ => None,
        }
    }
}

impl ToolSlot {
    /// Parse one stored slot; unknown fields keep their JSON values through
    /// the raw map.
    pub fn from_json(value: &Value) -> Option<ToolSlot> {
        let object = value.as_object()?;
        Some(ToolSlot {
            call_id: object.get("callId").and_then(Value::as_str)?.to_string(),
            name: object.get("name").and_then(Value::as_str)?.to_string(),
            task_id: object.get("taskId").and_then(Value::as_i64),
            status: SlotStatus::parse(object.get("status").and_then(Value::as_str)?)?,
            output: object
                .get("output")
                .and_then(Value::as_str)
                .map(str::to_string),
            dropped_bytes: object.get("droppedBytes").and_then(Value::as_f64),
            dropped_lines: object.get("droppedLines").and_then(Value::as_f64),
            details: object.get("details").cloned(),
            diagnostics: object.get("diagnostics").and_then(Value::as_array).cloned(),
            entry: object.get("entry").and_then(Value::as_i64),
        })
    }

    /// Serialize in the upstream construction order.
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert(String::from("callId"), Value::from(self.call_id.clone()));
        map.insert(String::from("name"), Value::from(self.name.clone()));
        if let Some(task_id) = self.task_id {
            map.insert(String::from("taskId"), Value::from(task_id));
        }
        map.insert(String::from("status"), Value::from(self.status.as_str()));
        if let Some(output) = &self.output {
            map.insert(String::from("output"), Value::from(output.clone()));
        }
        if let Some(dropped_bytes) = self.dropped_bytes {
            map.insert(
                String::from("droppedBytes"),
                super::usage::js_number_value(dropped_bytes),
            );
        }
        if let Some(dropped_lines) = self.dropped_lines {
            map.insert(
                String::from("droppedLines"),
                super::usage::js_number_value(dropped_lines),
            );
        }
        if let Some(details) = &self.details {
            map.insert(String::from("details"), details.clone());
        }
        if let Some(diagnostics) = &self.diagnostics {
            map.insert(
                String::from("diagnostics"),
                Value::Array(diagnostics.clone()),
            );
        }
        if let Some(entry) = self.entry {
            map.insert(String::from("entry"), Value::from(entry));
        }
        Value::Object(map)
    }
}

/// Built-in live conversation state (`live.ts` `LiveState`).
pub fn live_doc() -> DocToken {
    define_doc(super::super::documents::DocDefinition {
        kind: String::from("pi.live"),
        version: 1,
        scope: DefinitionScope::Conversation,
        history: Some(DocumentHistory::Latest),
        fork: Some(DocumentFork::Initial),
        family: false,
        initial: Arc::new(|_| Map::new()),
        migrate: None,
        // REMINDER (upstream note): a complete base whenever nothing runs
        // (spec §8.2): no generation and no running tool slot. Do not add a
        // delta-count bound; the tool output benchmark checks this rule.
        // Absent (`undefined`) and JSON-`null` generations both read as "no
        // generation", like the upstream `=== undefined` check reads a
        // deleted or never-set property.
        checkpoint_when: Some(Arc::new(|value, _, _| {
            let generation_idle = value
                .get("generation")
                .is_none_or(|generation| generation.is_null());
            let slots_idle = !value
                .get("tools")
                .and_then(Value::as_array)
                .is_some_and(|slots| {
                    slots
                        .iter()
                        .any(|slot| slot.get("status").and_then(Value::as_str) == Some("running"))
                });
            generation_idle && slots_idle
        })),
    })
    .expect("the built-in live document definition is valid")
}

/// Read the live value out of a draft (`None` before its first write).
pub fn read_live(draft: &DocumentDraft) -> Result<Option<Value>, PlainError> {
    draft
        .read(&[])
        .map_err(|error| PlainError::new(error.message().to_string()))
}

/// Built-in task kinds that can own `pi.live.run` (`RUN_TASK_KINDS`).
pub const RUN_TASK_KINDS: [&str; 1] = ["pi.generation"];
/// The built-in tool task kind (`TOOL_TASK_KIND`).
pub const TOOL_TASK_KIND: &str = "pi.tool";
/// The built-in compaction task kind (`COMPACTION_TASK_KIND`, v1.0.0).
pub const COMPACTION_TASK_KIND: &str = "pi.compaction";

/// Presentation of one live compaction task (spec §8.7) — `live.ts`
/// `CompactionStatus` (v1.0.0). Wire order
/// `{taskId, reason, blocking, attempt, retry?}`.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionStatus {
    pub task_id: TaskId,
    pub reason: super::types::CompactionReason,
    /// Whether a generation waits for it: a compaction the generation owns.
    pub blocking: bool,
    pub attempt: i64,
    /// Durable backoff before the next summarization attempt.
    pub retry: Option<CompactionRetry>,
}

/// `CompactionStatus.retry` (`{ at, error }`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionRetry {
    /// Epoch millis of the next attempt.
    pub at: i64,
    pub error: String,
}

impl CompactionStatus {
    /// Parse one stored status; unknown fields keep their JSON values through
    /// the raw map.
    pub fn from_json(value: &Value) -> Option<CompactionStatus> {
        let object = value.as_object()?;
        Some(CompactionStatus {
            task_id: object.get("taskId").and_then(Value::as_i64)?,
            reason: super::types::CompactionReason::deserialize(object.get("reason")?.clone())
                .ok()?,
            blocking: object.get("blocking").and_then(Value::as_bool)?,
            attempt: object.get("attempt").and_then(Value::as_i64)?,
            retry: object.get("retry").and_then(|retry| {
                let retry = retry.as_object()?;
                Some(CompactionRetry {
                    at: retry.get("at").and_then(Value::as_i64)?,
                    error: retry.get("error").and_then(Value::as_str)?.to_string(),
                })
            }),
        })
    }

    /// Serialize in the upstream construction order.
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert(String::from("taskId"), Value::from(self.task_id));
        map.insert(String::from("reason"), Value::from(self.reason.as_str()));
        map.insert(String::from("blocking"), Value::from(self.blocking));
        map.insert(String::from("attempt"), Value::from(self.attempt));
        if let Some(retry) = &self.retry {
            let mut retry_map = Map::new();
            retry_map.insert(String::from("at"), Value::from(retry.at));
            retry_map.insert(String::from("error"), Value::from(retry.error.clone()));
            map.insert(String::from("retry"), Value::Object(retry_map));
        }
        Value::Object(map)
    }
}

/// Add the status of a compaction task created in this commit (`addCompactionStatus`):
/// statuses stay in task ID order.
pub fn add_compaction_status(
    draft: &DocumentDraft,
    status: &CompactionStatus,
) -> Result<(), PlainError> {
    let mut statuses = read_live(draft)?
        .and_then(|live| live.get("compactions").cloned())
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    statuses.push(status.to_json());
    draft
        .set(
            &[Seg::Key(String::from("compactions"))],
            Value::Array(statuses),
        )
        .map_err(error_of)
}

/// The status of compaction task `taskId`, if listed (`compactionStatus`).
pub fn compaction_status(
    draft: &DocumentDraft,
    task_id: TaskId,
) -> Result<Option<CompactionStatus>, PlainError> {
    Ok(compaction_statuses(draft)?
        .into_iter()
        .find(|status| status.task_id == task_id))
}

/// The parsed `compactions` array of a live draft (`live.compactions ?? []`).
pub fn compaction_statuses(draft: &DocumentDraft) -> Result<Vec<CompactionStatus>, PlainError> {
    Ok(read_live(draft)?
        .and_then(|live| live.get("compactions").cloned())
        .and_then(|value| value.as_array().cloned())
        .map(|statuses| {
            statuses
                .iter()
                .filter_map(CompactionStatus::from_json)
                .collect()
        })
        .unwrap_or_default())
}

/// The draft path of one compaction status field
/// (`["compactions", index, field...]`).
fn compaction_path(index: usize, field: &[&str]) -> Vec<Seg> {
    let mut path = vec![Seg::Key(String::from("compactions")), Seg::Index(index)];
    path.extend(field.iter().map(|name| Seg::Key((*name).to_owned())));
    path
}

/// Remove the status of compaction task `taskId`, and the list once empty
/// (`removeCompactionStatus`).
pub fn remove_compaction_status(draft: &DocumentDraft, task_id: TaskId) -> Result<(), PlainError> {
    let statuses = read_live(draft)?
        .and_then(|live| live.get("compactions").cloned())
        .and_then(|value| value.as_array().cloned());
    let Some(statuses) = statuses else {
        return Ok(());
    };
    let index = statuses
        .iter()
        .position(|status| status.get("taskId").and_then(Value::as_i64) == Some(task_id));
    let Some(index) = index else {
        return Ok(());
    };
    if statuses.len() == 1 {
        return draft
            .delete(&[Seg::Key(String::from("compactions"))])
            .map_err(error_of);
    }
    draft.delete(&compaction_path(index, &[])).map_err(error_of)
}

type Seg = crate::chord::delta::Seg;

fn error_of(error: crate::chord::delta::TrackerError) -> PlainError {
    PlainError::new(error.message())
}

/// The stored `run` of a live value (`{taskId, inputs}`), if present.
pub fn run_of(live: &JsonObject) -> Option<(TaskId, Vec<SubmissionId>)> {
    let run = live.get("run").and_then(Value::as_object)?;
    let task_id = run.get("taskId").and_then(Value::as_i64)?;
    let inputs = run
        .get("inputs")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default();
    Some((task_id, inputs))
}

/// The stored `run` of a live draft, if present.
pub fn run_of_draft(
    draft: &DocumentDraft,
) -> Result<Option<(TaskId, Vec<SubmissionId>)>, PlainError> {
    Ok(read_live(draft)?
        .and_then(|value| value.as_object().cloned())
        .and_then(|live| run_of(&live)))
}

/// The `generation` object of a live draft, if present.
pub fn generation_of_draft(draft: &DocumentDraft) -> Result<Option<JsonObject>, PlainError> {
    Ok(read_live(draft)?
        .and_then(|generation| generation.get("generation").cloned())
        .and_then(|value| value.as_object().cloned()))
}

/// Set `generation` to one whole object (`live.generation = {...}`).
pub fn set_generation(draft: &DocumentDraft, value: JsonObject) -> Result<(), PlainError> {
    draft
        .set(
            &[Seg::Key(String::from("generation"))],
            Value::Object(value),
        )
        .map_err(error_of)
}

/// Remove `generation` (`delete live.generation`).
pub fn delete_generation(draft: &DocumentDraft) -> Result<(), PlainError> {
    draft
        .delete(&[Seg::Key(String::from("generation"))])
        .map_err(error_of)
}

/// Replace the `tools` array with the given slots (`live.tools = slots`).
pub fn set_tools(draft: &DocumentDraft, slots: &[ToolSlot]) -> Result<(), PlainError> {
    draft
        .set(
            &[Seg::Key(String::from("tools"))],
            Value::Array(slots.iter().map(ToolSlot::to_json).collect()),
        )
        .map_err(error_of)
}

/// Remove `tools` (`delete live.tools`).
pub fn delete_tools(draft: &DocumentDraft) -> Result<(), PlainError> {
    draft
        .delete(&[Seg::Key(String::from("tools"))])
        .map_err(error_of)
}

/// The parsed `tools` array of a live draft (`live.tools ?? []`).
pub fn tools_of_draft(draft: &DocumentDraft) -> Result<Vec<ToolSlot>, PlainError> {
    Ok(read_live(draft)?
        .and_then(|value| value.get("tools").cloned())
        .and_then(|value| Value::as_array(&value).cloned())
        .map(|slots| slots.iter().filter_map(ToolSlot::from_json).collect())
        .unwrap_or_default())
}

/// The draft path of one slot field (`["tools", index, field...]`).
pub fn slot_path(index: usize, field: &[&str]) -> Vec<Seg> {
    let mut path = vec![Seg::Key(String::from("tools")), Seg::Index(index)];
    path.extend(field.iter().map(|name| Seg::Key((*name).to_owned())));
    path
}

/// The index of tool task `taskId` in the current round (`toolSlot`), if the
/// round still lists it.
pub fn slot_index_of(draft: &DocumentDraft, task_id: TaskId) -> Result<Option<usize>, PlainError> {
    Ok(tools_of_draft(draft)?
        .iter()
        .position(|slot| slot.task_id == Some(task_id)))
}

/// Mark slot `index` running (`slot.status = "running"`).
pub fn set_slot_running(draft: &DocumentDraft, index: usize) -> Result<(), PlainError> {
    draft
        .set(&slot_path(index, &["status"]), Value::from("running"))
        .map_err(error_of)
}

/// Bind slot `index` to its started tool task (`slot.taskId = taskId`).
pub fn set_slot_task_id(
    draft: &DocumentDraft,
    index: usize,
    task_id: TaskId,
) -> Result<(), PlainError> {
    draft
        .set(&slot_path(index, &["taskId"]), Value::from(task_id))
        .map_err(error_of)
}

/// Write one slot field of a running slot (progress commits).
pub fn set_slot_field(
    draft: &DocumentDraft,
    index: usize,
    field: &str,
    value: Value,
) -> Result<(), PlainError> {
    draft
        .set(&slot_path(index, &[field]), value)
        .map_err(error_of)
}

/// Delete one slot field (`delete slot.<field>`).
pub fn delete_slot_field(
    draft: &DocumentDraft,
    index: usize,
    field: &str,
) -> Result<(), PlainError> {
    draft.delete(&slot_path(index, &[field])).map_err(error_of)
}

/// Append diagnostics to slot `index`
/// (`slot.diagnostics ??= []; slot.diagnostics.push(...)`).
pub fn push_slot_diagnostics(
    draft: &DocumentDraft,
    index: usize,
    diagnostics: Vec<Value>,
) -> Result<(), PlainError> {
    if diagnostics.is_empty() {
        return Ok(());
    }
    let path = slot_path(index, &["diagnostics"]);
    let existing = draft
        .read(&path)
        .map_err(error_of)?
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    let mut next = existing;
    next.extend(diagnostics);
    draft.set(&path, Value::Array(next)).map_err(error_of)
}

/// Mark slot `index` done (`finishSlot`): the result entry, if any, now
/// carries its running output, details, and diagnostics.
pub fn finish_slot(
    draft: &DocumentDraft,
    index: usize,
    entry: Option<EntryId>,
) -> Result<(), PlainError> {
    draft
        .set(&slot_path(index, &["status"]), Value::from("done"))
        .map_err(error_of)?;
    if let Some(entry) = entry {
        draft
            .set(&slot_path(index, &["entry"]), Value::from(entry))
            .map_err(error_of)?;
    }
    clear_progress(draft, index)
}

/// Remove what a tool published while running (`clearProgress`); its result
/// entry or a rerun replaces it.
pub fn clear_progress(draft: &DocumentDraft, index: usize) -> Result<(), PlainError> {
    for field in [
        "output",
        "droppedBytes",
        "droppedLines",
        "details",
        "diagnostics",
    ] {
        draft
            .delete(&slot_path(index, &[field]))
            .map_err(error_of)?;
    }
    Ok(())
}

/// End the run owned by `taskId` (`live.ts` `endRun`): settle each of its
/// inputs and remove `run`. Always removes `generation` and `tools`, whose
/// presentation belongs to the ending run.
pub fn end_run(
    tx: &Transaction,
    draft: &DocumentDraft,
    task_id: TaskId,
    settlement: SubmissionSettlement,
) -> Result<(), PlainError> {
    if let Some((run_task_id, inputs)) = run_of_draft(draft)? {
        if run_task_id == task_id {
            for id in inputs {
                tx.settle_submission(id, settlement.clone())?;
            }
            draft
                .delete(&[Seg::Key(String::from("run"))])
                .map_err(error_of)?;
        }
    }
    delete_generation(draft)?;
    delete_tools(draft)
}

/// Harness cleanup for a terminal outcome the scheduler writes itself
/// (`faulted` or `orphaned`, `live.ts` `settleSchedulerOutcome`). A run task
/// ends its run; a tool task's slot is marked done without an entry, and
/// context derivation synthesizes the missing result; a compaction task's
/// status is removed (v1.0.0). Ignores other kinds so it never creates
/// `pi.live` elsewhere. The scheduler calls this without knowing task kinds;
/// the Harness passes it in (spec §5.4).
/// REMINDER: a committed generation partial becomes an aborted assistant
/// entry here, exactly as in the generation abort handler, so the transcript
/// keeps what the model produced and `pi.usage` counts its spend. The
/// scheduler's commit has no task scope, so that entry has no `byTaskId`.
pub fn settle_scheduler_outcome(
    tx: &Transaction,
    record: &TaskRecord,
    outcome: &TaskOutcome,
) -> Result<(), PlainError> {
    let live = live_doc();
    let draft = tx.doc(&live.definition, Some(record.conversation_id), None, None)?;
    if record.kind == TOOL_TASK_KIND {
        if let Some(index) = slot_index_of(&draft, record.id)? {
            finish_slot(&draft, index, None)?;
        }
        return Ok(());
    }
    if record.kind == COMPACTION_TASK_KIND {
        return remove_compaction_status(&draft, record.id);
    }
    if !RUN_TASK_KINDS.contains(&record.kind.as_str()) {
        return Ok(());
    }
    if run_of_draft(&draft)?.map(|(task_id, _)| task_id) != Some(record.id) {
        return Ok(());
    }
    super::generation::convert_partial(tx, &draft, record.conversation_id)?;
    let settlement = match outcome {
        TaskOutcome::Faulted { error } => SubmissionSettlement::Unanswered {
            reason: String::from("faulted"),
            detail: Some(Value::String(error.message.clone())),
        },
        TaskOutcome::Orphaned { reason } => SubmissionSettlement::Unanswered {
            reason: reason.clone(),
            detail: None,
        },
        _ => return Ok(()),
    };
    end_run(tx, &draft, record.id, settlement)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable::harness::types::CompactionReason;

    #[test]
    fn compaction_status_round_trips_in_upstream_field_order() {
        let status = CompactionStatus {
            task_id: 7,
            reason: CompactionReason::Manual,
            blocking: true,
            attempt: 1,
            retry: None,
        };
        let json = status.to_json();
        let keys: Vec<&str> = json
            .as_object()
            .map(|map| map.keys().map(String::as_str).collect())
            .unwrap_or_default();
        assert_eq!(keys, vec!["taskId", "reason", "blocking", "attempt"]);
        assert_eq!(CompactionStatus::from_json(&json), Some(status.clone()));

        let retrying = CompactionStatus {
            retry: Some(CompactionRetry {
                at: 42,
                error: String::from("overloaded"),
            }),
            ..status
        };
        let json = retrying.to_json();
        let keys: Vec<&str> = json
            .as_object()
            .map(|map| map.keys().map(String::as_str).collect())
            .unwrap_or_default();
        assert_eq!(
            keys,
            vec!["taskId", "reason", "blocking", "attempt", "retry"]
        );
        assert_eq!(CompactionStatus::from_json(&json), Some(retrying));
        assert_eq!(CompactionStatus::from_json(&Value::Null), None);
    }
}
