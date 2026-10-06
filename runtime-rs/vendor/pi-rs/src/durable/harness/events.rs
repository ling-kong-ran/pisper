//! Port of `src/harness/events.ts`: the experimental agent event stream of
//! one conversation (spec §9.4), shaped like the coding agent's session
//! events — a snapshot at attachment, then one batch per commit translated
//! from the mount's view operations and the publication's table changes.
//!
//! Divergences (structural, disclosed):
//! - **D28 (events are JSON values).** Upstream `AgentEvent` is a TS object
//!   union; the port represents every event as a strict-JSON value in the
//!   upstream construction order ([`AgentEvent`]), so the delivered payloads
//!   serialize byte-for-byte. Field presence (`run`/`generation`/`entry`...)
//!   follows the upstream spread semantics.
//! - `watchEvents` runs over the facade's [`ConversationViews`] and the
//!   Session's storage directly (upstream reads the harness object).

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::agent_core::chord_support::context::Context;
use crate::ai::types::Message;

use super::super::errors::PlainError;
use super::super::harness::config::ConversationConfigState;
use super::super::harness::types::ToolDiagnostic;
use super::super::harness::usage::{initial_json as usage_initial, UsageState};
use super::super::ids::{ConversationId, EntryId, SubmissionId, TaskId};
use super::super::session::observation::CommittedWatch;
use super::super::session::session::Session;
use super::super::storage::Storage;
use super::super::types::{
    CommitChange, CommitPublication, EntryRecord, TableCommitChange, TaskOutcome, TaskState,
    WatchEnd,
};
use super::view::{ConversationView, ConversationViews, ViewObserver};

/// One change to the in-flight assistant message, relative to that message
/// (`MessageChange`), as a wire object `{type, contentIndex, block|delta|...}`.
#[derive(Debug, Clone, PartialEq)]
pub struct MessageChange(pub Value);

/// An experimental agent event (`AgentEvent`), serialized in the upstream
/// construction order.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentEvent(pub Value);

/// The typed parts of a view the events read (`Parts`).
struct Parts {
    live: Map<String, Value>,
    inbox: Option<Map<String, Value>>,
    config: Option<Map<String, Value>>,
    usage: Option<Map<String, Value>>,
}

fn parts(view: &Value) -> Parts {
    let view = ConversationView::from_json(view);
    Parts {
        live: view
            .docs
            .get("pi.live")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
        inbox: view
            .docs
            .get("pi.inbox")
            .and_then(Value::as_object)
            .cloned(),
        config: view
            .docs
            .get("pi.conversation.config")
            .and_then(Value::as_object)
            .cloned(),
        usage: view
            .docs
            .get("pi.usage")
            .and_then(Value::as_object)
            .cloned(),
    }
}

/// Queued inbox items of a view (`queued`): `{id, mode}` pairs.
fn queued(inbox: Option<&Map<String, Value>>) -> Value {
    let items = inbox
        .and_then(|inbox| inbox.get("items"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Value::Array(
        items
            .iter()
            .filter_map(|item| {
                let mut pair = Map::new();
                pair.insert(String::from("id"), item.get("id")?.clone());
                pair.insert(String::from("mode"), item.get("mode")?.clone());
                Some(Value::Object(pair))
            })
            .collect(),
    )
}

/// The attachment snapshot event (`snapshotOf`).
fn snapshot_of(view: &Value) -> AgentEvent {
    let state = parts(view);
    let mut map = Map::new();
    map.insert(String::from("type"), Value::from("snapshot"));
    map.insert(
        String::from("entries"),
        view.get("entries")
            .cloned()
            .unwrap_or(Value::Array(Vec::new())),
    );
    if let Some(run) = state.live.get("run") {
        let inputs = run
            .get("inputs")
            .cloned()
            .unwrap_or(Value::Array(Vec::new()));
        let mut run_object = Map::new();
        run_object.insert(String::from("inputs"), inputs);
        map.insert(String::from("run"), Value::Object(run_object));
    }
    if let Some(generation) = state.live.get("generation") {
        map.insert(String::from("generation"), generation.clone());
    }
    map.insert(
        String::from("tools"),
        state
            .live
            .get("tools")
            .cloned()
            .unwrap_or(Value::Array(Vec::new())),
    );
    map.insert(String::from("inbox"), queued(state.inbox.as_ref()));
    map.insert(
        String::from("config"),
        match &state.config {
            Some(config) => Value::Object(config.clone()),
            None => ConversationConfigState::initial()
                .into_json()
                .map(Value::Object)
                .unwrap_or(Value::Null),
        },
    );
    map.insert(
        String::from("usage"),
        match &state.usage {
            Some(usage) => Value::Object(usage.clone()),
            None => Value::Object(usage_initial()),
        },
    );
    AgentEvent(Value::Object(map))
}

/// The batch listener (`AgentEventStream.start`).
pub type EventListener = Arc<
    dyn Fn(
            Vec<AgentEvent>,
            Context,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

/// Serialized stream of one conversation's event batches, one per commit
/// (`AgentEventStream`).
pub struct AgentEventStream {
    /// The `snapshot` event at attachment.
    pub snapshot: AgentEvent,
    watch: Arc<CommittedWatch>,
}

impl AgentEventStream {
    /// Install the sole asynchronous listener (`start`).
    pub fn start(&self, listener: EventListener) {
        self.watch.start(Arc::new(move |value, _ops, context| {
            let events = decode_events(&value);
            let listener = Arc::clone(&listener);
            Box::pin(async move { listener(events, context).await })
        }));
    }

    /// Idempotently stop future callbacks (`stop`).
    pub fn stop(&self) -> WatchEnd {
        self.watch.stop()
    }

    /// Settle when the stream terminates (`closed`).
    pub async fn closed(&self) -> WatchEnd {
        self.watch.closed().await
    }

    /// Observe one cancellation token (`observeCancellation` through the
    /// acquisition context).
    pub fn observe_cancellation(&self, token: CancellationToken) {
        self.watch.observe_cancellation(token);
    }
}

/// Decode a watch value carrying one batch of events.
fn decode_events(value: &Value) -> Vec<AgentEvent> {
    value
        .as_array()
        .map(|events| {
            events
                .iter()
                .map(|event| AgentEvent(event.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// Experimental: attach to one conversation's agent events (spec §9.4,
/// `watchEvents`). The snapshot and the registration for later commits are
/// captured atomically on the Session line; overflow replaces undelivered
/// batches with one snapshot.
pub async fn watch_events(
    views: &Arc<ConversationViews>,
    storage: &Arc<dyn Storage>,
    conversation_id: ConversationId,
    context: Context,
) -> Result<AgentEventStream, PlainError> {
    let current: Arc<std::sync::Mutex<Value>> = Arc::new(std::sync::Mutex::new(Value::Null));
    let held: Arc<std::sync::Mutex<BTreeSet<TaskId>>> = Arc::new(std::sync::Mutex::new(
        EventsObserver::held_completing(storage, conversation_id, &context),
    ));
    let watch = Arc::new(CommittedWatch::new(
        Value::Array(Vec::new()),
        Box::new(|| {}),
        Some(Box::new({
            let current = Arc::clone(&current);
            move || {
                let snapshot = snapshot_of(
                    &current
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()),
                );
                Value::Array(vec![snapshot.0])
            }
        })),
    ));
    let observer: Arc<dyn ViewObserver> = Arc::new(EventsObserver {
        conversation_id,
        storage: Arc::clone(storage),
        current: Arc::clone(&current),
        held: Arc::clone(&held),
        watch: Arc::clone(&watch),
        context: context.clone(),
    });
    let initial_view = views.attach(conversation_id, observer, &context)?;
    let snapshot = snapshot_of(&initial_view);
    *current
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = initial_view;
    // Like a watch, the acquisition context governs the stream's lifetime.
    let signal = context.abort_signal();
    if signal.as_ref().is_some_and(|signal| signal.is_cancelled()) {
        watch.cancel();
        return Err(PlainError::new("The operation was aborted"));
    }
    if let Some(signal) = &signal {
        watch.observe_cancellation(signal.clone());
    }
    Ok(AgentEventStream { snapshot, watch })
}

/// The mount observer that translates every publication into an event batch
/// (`attach`'s create callback).
struct EventsObserver {
    conversation_id: ConversationId,
    // Committed-task reads for the attach-time held-generation scan.
    #[allow(dead_code)]
    storage: Arc<dyn Storage>,
    current: Arc<std::sync::Mutex<Value>>,
    held: Arc<std::sync::Mutex<BTreeSet<TaskId>>>,
    watch: Arc<CommittedWatch>,
    // Publication contexts carry no caller cancellation (D25).
    #[allow(dead_code)]
    context: Context,
}

impl EventsObserver {
    /// Generations whose held outcome already ended their turn, read on the
    /// line with the snapshot.
    fn held_completing(
        storage: &Arc<dyn Storage>,
        conversation_id: ConversationId,
        context: &Context,
    ) -> BTreeSet<TaskId> {
        let mut held = BTreeSet::new();
        let mut cursor = None;
        loop {
            let page = storage
                .scan_tasks(
                    super::super::types::TaskQuery {
                        conversation_id: Some(conversation_id),
                        kind: Some(String::from("pi.generation")),
                        status: Some(super::super::types::TaskStatus::Completing),
                        ..Default::default()
                    },
                    100,
                    cursor.as_ref(),
                    context,
                )
                .unwrap_or_else(|_| super::super::types::Page::empty());
            let next = page.next.clone();
            for record in page.items {
                held.insert(record.id);
            }
            cursor = next;
            if cursor.is_none() {
                break;
            }
        }
        held
    }
}

impl ViewObserver for EventsObserver {
    fn advance(&self, _value: &Value, _ops: &[crate::chord::delta::Op], _context: &Context) {
        // The events watch is driven through `publication`; a pure revision
        // push carries no batch.
    }

    fn publication(
        &self,
        before: &Value,
        after: &Value,
        ops: &[crate::chord::delta::Op],
        publication: &CommitPublication,
        context: &Context,
    ) {
        *self
            .current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = after.clone();
        let events = translate(
            self.conversation_id,
            before,
            after,
            ops,
            publication,
            &self.held,
        );
        if !events.is_empty() {
            self.watch.advance(
                Value::Array(events.into_iter().map(|event| event.0).collect()),
                Vec::new(),
                context.clone(),
            );
        }
    }

    fn close_session(&self) {
        self.watch.close_session();
    }
}

/// Every event one publication causes, in the order of spec §9.4
/// (`translate`).
pub fn translate(
    conversation_id: ConversationId,
    before: &Value,
    after: &Value,
    view_ops: &[crate::chord::delta::Op],
    publication: &CommitPublication,
    held: &std::sync::Mutex<BTreeSet<TaskId>>,
) -> Vec<AgentEvent> {
    let mut entries: Vec<EntryRecord> = Vec::new();
    let mut tasks: Vec<super::super::types::TaskRecord> = Vec::new();
    let mut submissions: Vec<super::super::types::SubmissionRecord> = Vec::new();
    for change in &publication.changes {
        match change {
            CommitChange::Table(TableCommitChange::Entry { value }) => {
                if value.conversation_id == conversation_id {
                    entries.push(value.clone());
                }
            }
            CommitChange::Table(TableCommitChange::Task { value }) => {
                if value.conversation_id == conversation_id {
                    tasks.push(value.clone());
                }
            }
            CommitChange::Table(TableCommitChange::Submission { value })
                if value.conversation_id == conversation_id =>
            {
                submissions.push(value.clone());
            }
            _ => {}
        }
    }
    if view_ops.is_empty() && entries.is_empty() && tasks.is_empty() && submissions.is_empty() {
        return Vec::new();
    }
    // Entries are appended in ID order; submission records are published in
    // the order the commit first touched them.
    submissions.sort_by_key(|record| record.id);
    let was = parts(before);
    let now = parts(after);
    let mut events: Vec<AgentEvent> = Vec::new();

    // Progress: tool starts, the in-flight message, tool updates, retry and
    // deferred state.
    let slot_map = |live: &Map<String, Value>| -> Vec<(String, Map<String, Value>)> {
        live.get("tools")
            .and_then(Value::as_array)
            .map(|slots| {
                slots
                    .iter()
                    .filter_map(|slot| {
                        let call_id = slot.get("callId").and_then(Value::as_str)?.to_owned();
                        Some((call_id, slot.as_object().cloned().unwrap_or_default()))
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let slots_before = slot_map(&was.live);
    let slots = slot_map(&now.live);
    let task_of = |task_id: &str| -> Option<Map<String, Value>> {
        let task = tasks.iter().find(|task| task.id.to_string() == task_id)?;
        let wire = serde_json::to_value(task).ok()?;
        wire.as_object().cloned()
    };
    for slot in &slots {
        let status = slot.1.get("status").and_then(Value::as_str).unwrap_or("");
        let before_status = slots_before
            .iter()
            .find(|(call_id, _)| *call_id == slot.0)
            .and_then(|(_, previous)| previous.get("status"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if status != "running" || before_status == "running" {
            continue;
        }
        let checkpoint = slot
            .1
            .get("taskId")
            .and_then(Value::as_str)
            .and_then(&task_of)
            .and_then(|task| task.get("state").cloned())
            .and_then(|state| state.get("checkpoint").cloned());
        let args = checkpoint
            .as_ref()
            .and_then(|checkpoint| checkpoint.get("arguments").cloned())
            .filter(|arguments| arguments.is_object())
            .unwrap_or_else(|| Value::Object(Map::new()));
        let mut event = Map::new();
        event.insert(String::from("type"), Value::from("tool_execution_start"));
        event.insert(String::from("toolCallId"), Value::from(slot.0.clone()));
        event.insert(
            String::from("toolName"),
            slot.1.get("name").cloned().unwrap_or(Value::Null),
        );
        event.insert(String::from("args"), args);
        events.push(AgentEvent(Value::Object(event)));
    }
    let partial_before = was
        .live
        .get("generation")
        .and_then(|generation| generation.get("message"))
        .cloned();
    let partial = now
        .live
        .get("generation")
        .and_then(|generation| generation.get("message"))
        .cloned();
    if let Some(partial) = &partial {
        if partial_before.is_none() {
            let mut event = Map::new();
            event.insert(String::from("type"), Value::from("message_start"));
            event.insert(String::from("message"), partial.clone());
            events.push(AgentEvent(Value::Object(event)));
        } else if partial_before.as_ref() != Some(partial) {
            let usage = partial.get("usage").cloned().unwrap_or(Value::Null);
            let mut event = Map::new();
            event.insert(String::from("type"), Value::from("message_update"));
            event.insert(String::from("usage"), usage);
            event.insert(
                String::from("changes"),
                Value::Array(
                    message_changes(view_ops, partial)
                        .into_iter()
                        .map(|change| change.0.clone())
                        .collect(),
                ),
            );
            events.push(AgentEvent(Value::Object(event)));
        }
    }
    for (index, slot) in slots.iter().enumerate() {
        let Some((_, previous)) = slots_before.iter().find(|(call_id, _)| *call_id == slot.0)
        else {
            continue;
        };
        if slot.1.get("status").and_then(Value::as_str) != Some("running")
            || previous.get("status").and_then(Value::as_str) != Some("running")
        {
            continue;
        }
        if let Some(update) = tool_update(view_ops, index, slot.1.clone(), previous.clone()) {
            let mut event = Map::new();
            event.insert(String::from("type"), Value::from("tool_execution_update"));
            event.insert(String::from("toolCallId"), Value::from(slot.0.clone()));
            event.insert(
                String::from("toolName"),
                slot.1.get("name").cloned().unwrap_or(Value::Null),
            );
            for (key, value) in update {
                event.insert(key, value);
            }
            events.push(AgentEvent(Value::Object(event)));
        }
    }
    let generation = now
        .live
        .get("generation")
        .and_then(Value::as_object)
        .cloned();
    let generation_before = was
        .live
        .get("generation")
        .and_then(Value::as_object)
        .cloned();
    if let (Some(generation), Some(retry)) = (
        &generation,
        generation.as_ref().and_then(|g| g.get("retry")).cloned(),
    ) {
        if generation_before
            .as_ref()
            .and_then(|g| g.get("retry"))
            .is_none()
        {
            let mut event = Map::new();
            event.insert(String::from("type"), Value::from("auto_retry_start"));
            event.insert(
                String::from("attempt"),
                generation.get("attempt").cloned().unwrap_or(Value::Null),
            );
            event.insert(
                String::from("at"),
                retry.get("at").cloned().unwrap_or(Value::Null),
            );
            event.insert(
                String::from("errorMessage"),
                retry.get("error").cloned().unwrap_or(Value::Null),
            );
            events.push(AgentEvent(Value::Object(event)));
        }
    }
    if let Some(retry_before) = generation_before
        .as_ref()
        .and_then(|g| g.get("retry"))
        .cloned()
    {
        if generation.as_ref().and_then(|g| g.get("retry")).is_none() {
            let mut event = Map::new();
            event.insert(String::from("type"), Value::from("auto_retry_end"));
            event.insert(
                String::from("attempt"),
                generation_before
                    .as_ref()
                    .and_then(|g| g.get("attempt"))
                    .cloned()
                    .unwrap_or(Value::Null),
            );
            let _ = retry_before;
            events.push(AgentEvent(Value::Object(event)));
        }
    }
    let deferred_poll = generation
        .as_ref()
        .and_then(|g| g.get("deferred"))
        .and_then(|deferred| deferred.get("pollAt"))
        .cloned();
    let deferred_poll_before = generation_before
        .as_ref()
        .and_then(|g| g.get("deferred"))
        .and_then(|deferred| deferred.get("pollAt"))
        .cloned();
    if let Some(poll_at) = deferred_poll.clone() {
        if deferred_poll.as_ref() != deferred_poll_before.as_ref() {
            let mut event = Map::new();
            event.insert(String::from("type"), Value::from("deferred_poll"));
            event.insert(String::from("pollAt"), poll_at);
            events.push(AgentEvent(Value::Object(event)));
        }
    }

    // Tools that end in this commit: a slot that becomes done, one created
    // done (a call not offered), or an unfinished one that vanishes because
    // its run ended.
    struct ToolEnd {
        call_id: String,
        name: Value,
        entry: Option<Value>,
    }
    let mut tool_ends: Vec<ToolEnd> = Vec::new();
    let entry_by_id = |entry_id: Option<Value>| -> Option<Value> {
        let entry_id = entry_id?.as_i64()?;
        entries
            .iter()
            .find(|entry| entry.id == entry_id)
            .and_then(|entry| serde_json::to_value(entry).ok())
    };
    for (call_id, previous) in &slots_before {
        let previous_status = previous.get("status").and_then(Value::as_str).unwrap_or("");
        if previous_status == "done" {
            continue;
        }
        let slot = slots
            .iter()
            .find(|(candidate, _)| candidate == call_id)
            .map(|(found, slot)| (found.clone(), slot.clone()));
        match slot {
            Some((_, slot)) if slot.get("status").and_then(Value::as_str) == Some("done") => {
                tool_ends.push(ToolEnd {
                    call_id: call_id.clone(),
                    name: previous.get("name").cloned().unwrap_or(Value::Null),
                    entry: entry_by_id(slot.get("entry").cloned()),
                });
            }
            // A slot whose run ended in this commit may have had its result
            // appended with it, as for unstarted calls.
            None => {
                let entry = result_of(&entries, call_id);
                tool_ends.push(ToolEnd {
                    call_id: call_id.clone(),
                    name: previous.get("name").cloned().unwrap_or(Value::Null),
                    entry: entry.map(|entry| serde_json::to_value(entry).unwrap_or(Value::Null)),
                });
            }
            _ => {}
        }
    }
    for (call_id, slot) in &slots {
        if slot.get("status").and_then(Value::as_str) == Some("done")
            && !slots_before
                .iter()
                .any(|(before_call_id, _)| before_call_id == call_id)
        {
            tool_ends.push(ToolEnd {
                call_id: call_id.clone(),
                name: slot.get("name").cloned().unwrap_or(Value::Null),
                entry: entry_by_id(slot.get("entry").cloned()),
            });
        }
    }

    // Entries in append order; a tool's end directly precedes its result's
    // message, as in the coding agent.
    let entry_wire = |entry: &EntryRecord| serde_json::to_value(entry).unwrap_or(Value::Null);
    let mut assistant_appended = false;
    for entry in &entries {
        let wire = entry_wire(entry);
        let ends: Vec<&ToolEnd> = tool_ends
            .iter()
            .filter(|end| end.entry.as_ref() == Some(&wire))
            .collect();
        for end in ends {
            let mut event = Map::new();
            event.insert(String::from("type"), Value::from("tool_execution_end"));
            event.insert(String::from("toolCallId"), Value::from(end.call_id.clone()));
            event.insert(String::from("toolName"), end.name.clone());
            if let Some(entry) = &end.entry {
                event.insert(String::from("entry"), entry.clone());
            }
            events.push(AgentEvent(Value::Object(event)));
        }
        let message = entry.model.as_ref().and_then(|model| model.first());
        let Some(message) = message else {
            let mut event = Map::new();
            event.insert(String::from("type"), Value::from("entry_appended"));
            event.insert(String::from("entry"), wire);
            events.push(AgentEvent(Value::Object(event)));
            continue;
        };
        // A streamed answer already started with its first partial.
        let streamed = matches!(message, Message::Assistant(_))
            && partial_before.is_some()
            && !assistant_appended;
        if matches!(message, Message::Assistant(_)) {
            assistant_appended = true;
        }
        if !streamed {
            let mut event = Map::new();
            event.insert(String::from("type"), Value::from("message_start"));
            event.insert(
                String::from("message"),
                serde_json::to_value(message).unwrap_or(Value::Null),
            );
            events.push(AgentEvent(Value::Object(event)));
        }
        let mut event = Map::new();
        event.insert(String::from("type"), Value::from("message_end"));
        event.insert(String::from("entry"), wire);
        events.push(AgentEvent(Value::Object(event)));
    }
    // Ends without a result entry: a faulted or orphaned tool, or one whose
    // run ended.
    for end in tool_ends.iter().filter(|end| end.entry.is_none()) {
        let mut event = Map::new();
        event.insert(String::from("type"), Value::from("tool_execution_end"));
        event.insert(String::from("toolCallId"), Value::from(end.call_id.clone()));
        event.insert(String::from("toolName"), end.name.clone());
        events.push(AgentEvent(Value::Object(event)));
    }

    // Task failures, then turn and run ends. A generation's turn ends when
    // its outcome is committed: at a `completing` hold or at terminal,
    // whichever comes first, so a successor created at the hold starts after
    // it.
    let mut turn_ended = false;
    for task in &tasks {
        let status = task.status();
        let outcome = match &task.state {
            TaskState::Completing { outcome } | TaskState::Terminal { outcome } => {
                Some(outcome.clone())
            }
            _ => None,
        };
        if task.kind == "pi.generation" && status == super::super::types::TaskStatus::Completing {
            let mut held = held.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if held.insert(task.id) {
                turn_ended = true;
            }
        }
        if status != super::super::types::TaskStatus::Terminal {
            continue;
        }
        if task.kind == "pi.generation"
            && !held
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&task.id)
        {
            turn_ended = true;
        }
        let Some(outcome) = outcome else { continue };
        if matches!(
            outcome,
            TaskOutcome::Faulted { .. } | TaskOutcome::Orphaned { .. }
        ) {
            let message = match &outcome {
                TaskOutcome::Faulted { error } => error.message.clone(),
                TaskOutcome::Orphaned { reason } => reason.clone(),
                _ => String::new(),
            };
            let mut event = Map::new();
            event.insert(String::from("type"), Value::from("task_failed"));
            event.insert(String::from("taskId"), Value::from(task.id));
            event.insert(String::from("kind"), Value::from(task.kind.clone()));
            event.insert(String::from("message"), Value::from(message));
            events.push(AgentEvent(Value::Object(event)));
        }
    }
    if turn_ended {
        let mut event = Map::new();
        event.insert(String::from("type"), Value::from("turn_end"));
        events.push(AgentEvent(Value::Object(event)));
    }
    let run = now.live.get("run").cloned();
    let run_before = was.live.get("run").cloned();
    let first_input = |run: Option<&Value>| {
        run.and_then(|run| run.get("inputs").and_then(Value::as_array))
            .and_then(|inputs| inputs.first())
            .cloned()
    };
    let run_changed = first_input(run.as_ref()) != first_input(run_before.as_ref());
    if let Some(run_before) = &run_before {
        if run_changed {
            let mut event = Map::new();
            event.insert(String::from("type"), Value::from("run_end"));
            event.insert(
                String::from("inputs"),
                run_before
                    .get("inputs")
                    .cloned()
                    .unwrap_or(Value::Array(Vec::new())),
            );
            events.push(AgentEvent(Value::Object(event)));
        }
    }

    // Submissions, document state, then what began.
    for record in &submissions {
        let mut event = Map::new();
        event.insert(String::from("type"), Value::from("submission"));
        event.insert(
            String::from("record"),
            serde_json::to_value(record).unwrap_or(Value::Null),
        );
        events.push(AgentEvent(Value::Object(event)));
    }
    if now.inbox != was.inbox {
        let mut event = Map::new();
        event.insert(String::from("type"), Value::from("inbox_update"));
        event.insert(String::from("items"), queued(now.inbox.as_ref()));
        events.push(AgentEvent(Value::Object(event)));
    }
    // A retired document reads as its initial value, as in a snapshot.
    if now.config != was.config {
        let mut event = Map::new();
        event.insert(String::from("type"), Value::from("config_changed"));
        event.insert(
            String::from("config"),
            match &now.config {
                Some(config) => Value::Object(config.clone()),
                None => ConversationConfigState::initial()
                    .into_json()
                    .map(Value::Object)
                    .unwrap_or(Value::Null),
            },
        );
        events.push(AgentEvent(Value::Object(event)));
    }
    if now.usage != was.usage {
        let mut event = Map::new();
        event.insert(String::from("type"), Value::from("usage_changed"));
        event.insert(
            String::from("usage"),
            match &now.usage {
                Some(usage) => Value::Object(usage.clone()),
                None => Value::Object(usage_initial()),
            },
        );
        events.push(AgentEvent(Value::Object(event)));
    }
    if let Some(run) = &run {
        if run_changed {
            let mut event = Map::new();
            event.insert(String::from("type"), Value::from("run_start"));
            event.insert(
                String::from("inputs"),
                run.get("inputs")
                    .cloned()
                    .unwrap_or(Value::Array(Vec::new())),
            );
            events.push(AgentEvent(Value::Object(event)));
        }
        let run_task_id = run.get("taskId").cloned();
        let run_task_id_before = run_before
            .as_ref()
            .and_then(|run| run.get("taskId").cloned());
        if run_task_id != run_task_id_before {
            let kind_is_generation = run_task_id
                .as_ref()
                .and_then(Value::as_i64)
                .map(|task_id| {
                    tasks
                        .iter()
                        .any(|task| task.id == task_id && task.kind == "pi.generation")
                })
                .unwrap_or(false);
            if kind_is_generation {
                let mut event = Map::new();
                event.insert(String::from("type"), Value::from("turn_start"));
                events.push(AgentEvent(Value::Object(event)));
            }
        }
    }
    events
}

/// The tool result for `call_id` among `entries` (`resultOf`).
fn result_of(entries: &[EntryRecord], call_id: &str) -> Option<EntryRecord> {
    entries
        .iter()
        .find(|entry| {
            entry
                .model
                .as_ref()
                .and_then(|model| model.first())
                .map(|message| match message {
                    Message::ToolResult(result) => result.tool_call_id == call_id,
                    _ => false,
                })
                .unwrap_or(false)
        })
        .cloned()
}

/// Translate the view operations on the in-flight message into message
/// changes (spec §9.4, `messageChanges`).
fn message_changes(view_ops: &[crate::chord::delta::Op], message: &Value) -> Vec<MessageChange> {
    const PARTIAL_PATH: [&str; 4] = ["docs", "pi.live", "generation", "message"];
    let segments_of = |names: &[&str]| -> Vec<crate::chord::delta::Seg> {
        names
            .iter()
            .map(|name| crate::chord::delta::Seg::Key((*name).to_owned()))
            .collect()
    };
    let partial_segments = segments_of(&PARTIAL_PATH);
    let mut changes: Vec<MessageChange> = Vec::new();
    let to_change = |pairs: Vec<(String, Value)>| -> MessageChange {
        MessageChange(Value::Object(pairs.into_iter().collect()))
    };
    // A block sent whole already holds every later change to it in this
    // batch.
    let mut whole: BTreeSet<usize> = BTreeSet::new();
    for op in view_ops {
        // View operations never replace the root.
        let Some(path) = op.path() else { continue };
        if !starts_with(path, &PARTIAL_PATH) {
            // The whole message or generation was replaced.
            if starts_with_segs(&partial_segments, path) {
                return vec![to_change(vec![
                    (String::from("type"), Value::from("message")),
                    (String::from("message"), message.clone()),
                ])];
            }
            continue;
        }
        let rest = &path[PARTIAL_PATH.len()..];
        match rest.first() {
            Some(crate::chord::delta::Seg::Key(key)) if key == "usage" => continue,
            Some(crate::chord::delta::Seg::Key(key)) if key == "content" => {}
            _ => {
                return vec![to_change(vec![
                    (String::from("type"), Value::from("message")),
                    (String::from("message"), message.clone()),
                ])];
            }
        }
        if rest.len() == 1 {
            let crate::chord::delta::Op::Splice {
                index,
                remove,
                items,
                ..
            } = op
            else {
                return vec![to_change(vec![
                    (String::from("type"), Value::from("message")),
                    (String::from("message"), message.clone()),
                ])];
            };
            if *remove != 0 {
                return vec![to_change(vec![
                    (String::from("type"), Value::from("message")),
                    (String::from("message"), message.clone()),
                ])];
            }
            for (offset, block) in items.iter().enumerate() {
                let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
                let change_type = match block_type {
                    "text" => "text_start",
                    "thinking" => "thinking_start",
                    _ => "toolcall_start",
                };
                let mut change = Map::new();
                change.insert(String::from("type"), Value::from(change_type));
                change.insert(String::from("contentIndex"), Value::from(index + offset));
                change.insert(String::from("block"), block.clone());
                changes.push(MessageChange(Value::Object(change)));
            }
            continue;
        }
        let content_index = match rest.get(1) {
            Some(crate::chord::delta::Seg::Index(index)) => *index,
            _ => {
                return vec![to_change(vec![
                    (String::from("type"), Value::from("message")),
                    (String::from("message"), message.clone()),
                ])];
            }
        };
        let field = match rest.get(2) {
            Some(crate::chord::delta::Seg::Key(key)) => key.as_str(),
            _ => "",
        };
        if whole.contains(&content_index) {
            continue;
        }
        match (op, field) {
            (crate::chord::delta::Op::Append { text, .. }, "text" | "thinking")
                if rest.len() == 3 =>
            {
                let mut change = Map::new();
                change.insert(
                    String::from("type"),
                    Value::from(if field == "text" {
                        "text_delta"
                    } else {
                        "thinking_delta"
                    }),
                );
                change.insert(String::from("contentIndex"), Value::from(content_index));
                change.insert(String::from("delta"), Value::from(text.clone()));
                changes.push(MessageChange(Value::Object(change)));
            }
            (crate::chord::delta::Op::Append { text, .. }, "arguments") => {
                let mut change = Map::new();
                change.insert(String::from("type"), Value::from("toolcall_delta"));
                change.insert(String::from("contentIndex"), Value::from(content_index));
                change.insert(
                    String::from("path"),
                    Value::Array(
                        rest.iter()
                            .skip(3)
                            .map(|segment| match segment {
                                crate::chord::delta::Seg::Key(key) => Value::from(key.clone()),
                                crate::chord::delta::Seg::Index(index) => Value::from(*index),
                            })
                            .collect(),
                    ),
                );
                change.insert(String::from("delta"), Value::from(text.clone()));
                changes.push(MessageChange(Value::Object(change)));
            }
            _ => {
                whole.insert(content_index);
                let block = message
                    .get("content")
                    .and_then(Value::as_array)
                    .and_then(|content| content.get(content_index))
                    .cloned()
                    .unwrap_or(Value::Null);
                let mut change = Map::new();
                change.insert(String::from("type"), Value::from("block"));
                change.insert(String::from("contentIndex"), Value::from(content_index));
                change.insert(String::from("block"), block);
                changes.push(MessageChange(Value::Object(change)));
            }
        }
    }
    changes
}

/// Output, details, and diagnostics changes of a running slot, from the view
/// operations on it (`toolUpdate`).
fn tool_update(
    view_ops: &[crate::chord::delta::Op],
    index: usize,
    slot: Map<String, Value>,
    previous: Map<String, Value>,
) -> Option<Vec<(String, Value)>> {
    let output_path = [
        crate::chord::delta::Seg::Key(String::from("docs")),
        crate::chord::delta::Seg::Key(String::from("pi.live")),
        crate::chord::delta::Seg::Key(String::from("tools")),
        crate::chord::delta::Seg::Index(index),
        crate::chord::delta::Seg::Key(String::from("output")),
    ];
    let mut trim_start = 0usize;
    let mut append = String::new();
    let mut set = false;
    for op in view_ops {
        let Some(path) = op.path() else { continue };
        if !starts_with_segs(path, &output_path) {
            continue;
        }
        match op {
            crate::chord::delta::Op::Truncate { count, .. } => trim_start += count,
            crate::chord::delta::Op::Append { text, .. } => append.push_str(text),
            _ => set = true,
        }
    }
    let slot_output = slot
        .get("output")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let previous_output = previous
        .get("output")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut update: Vec<(String, Value)> = Vec::new();
    let output_changed = slot_output != previous_output;
    if set || (output_changed && trim_start == 0 && append.is_empty()) {
        update.push((
            String::from("output"),
            Value::Object({
                let mut set_object = Map::new();
                set_object.insert(
                    String::from("set"),
                    Value::from(slot_output.clone().unwrap_or_default()),
                );
                set_object
            }),
        ));
    } else if trim_start > 0 || !append.is_empty() {
        let mut output = Map::new();
        if trim_start > 0 {
            output.insert(String::from("trimStart"), Value::from(trim_start));
        }
        if !append.is_empty() {
            output.insert(String::from("append"), Value::from(append));
        }
        update.push((String::from("output"), Value::Object(output)));
    }
    if slot.get("details") != previous.get("details") {
        update.push((
            String::from("details"),
            slot.get("details").cloned().unwrap_or(Value::Null),
        ));
    }
    if slot.get("diagnostics") != previous.get("diagnostics") {
        update.push((
            String::from("diagnostics"),
            slot.get("diagnostics")
                .cloned()
                .unwrap_or(Value::Array(Vec::new())),
        ));
    }
    if update.is_empty() {
        None
    } else {
        Some(update)
    }
}

fn starts_with(path: &[crate::chord::delta::Seg], prefix: &[&str]) -> bool {
    starts_with_segs(
        path,
        &prefix
            .iter()
            .map(|name| crate::chord::delta::Seg::Key((*name).to_owned()))
            .collect::<Vec<_>>(),
    )
}

fn starts_with_segs(
    path: &[crate::chord::delta::Seg],
    prefix: &[crate::chord::delta::Seg],
) -> bool {
    prefix.len() <= path.len()
        && prefix
            .iter()
            .zip(path.iter())
            .all(|(prefix, segment)| prefix == segment)
}

/// The typed parts re-export for the harness facade's stream surface.
pub type EventUsageState = UsageState;

/// A tool diagnostic re-export for the wire shape.
pub type EventToolDiagnostic = ToolDiagnostic;

/// Entry id re-export for the stream surface.
pub type EventEntryId = EntryId;

/// Submission id re-export for the stream surface.
pub type EventSubmissionId = SubmissionId;

/// Session re-export for the observer.
pub type EventSession = Session;
