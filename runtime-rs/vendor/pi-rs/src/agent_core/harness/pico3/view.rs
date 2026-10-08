//! Port of `packages/agent/src/harness/pico3/view.ts` (477 lines): the
//! commit-granular conversation view — [`ViewManager`], the [`Watch`]
//! lifecycle, and envelope application.
//!
//! Upstream tracks the view through a chord `Tracker` and emits minimal ops
//! per commit; the port keeps exactly that: the record's tracker walks the
//! JSON form of [`ConversationView`], `update` rebuilds only the affected
//! projections and diffs them through the tracker's flush, and `deliver`
//! hands the frozen envelope to watchers off the line. `applyEntries`
//! (`view.ts:337-354`) splices the transcript for head commits before the
//! tracked diff, preserving the upstream splice-op shapes
//! (`["p", ["entries"], 0, remove, []]`).
//!
//! Disclosed substitutions: the `syncRecord`/`syncArray`/`same` helpers
//! (`view.ts:413-477`) minimize emitted ops over JS objects; the port diffs
//! whole-value at flush with the chord delta's own diff functions (the
//! `Tracker` port — see `chord_support::delta`), which produces the same op
//! vocabulary with possibly different, equally valid op sequences (the delta
//! contract: consumers depend on the resulting value, not the exact tuples).
//! `structuredClone` is a serde round-trip.
//!
//! Wiring note (disclosed): upstream `harness.ts` registers the manager's
//! `update`/`deliver` on the Session's listener sets; that wiring lands with
//! the harness task (M3b Task 9). Until then callers invoke
//! [`ViewManager::update`] after a commit and [`ViewManager::deliver`]
//! after the line drains — matching the upstream call positions.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::agent_core::chord_support::delta::{apply_immutable, track, Op, Tracker};

use super::session::{CommitRecord, Session};
use super::types::{
    checkpoint_phase, Conversation, ConversationView, Entry, Envelope, GenerationStatus, Id,
    JsonObject, Task, TaskStatus, TurnView, ViewEvent,
};

/// Upstream `WATCH_CAPACITY` (`view.ts:21`).
pub const WATCH_CAPACITY: usize = 256;

/// Upstream `Watch` (`view.ts:23-29`): the observer handle. The listener is
/// shared (`Arc<dyn Fn>`) so `start` can accept it once; upstream enforces
/// one `start` per watch and the port keeps that check. Listener failures
/// (panics) stop the watch and report, never propagating
/// (`view.ts:317-334`).
pub struct Watch {
    state: Arc<Mutex<WatchState>>,
    /// The snapshot at watch creation (`view.ts:63`).
    pub view: ConversationView,
    /// The revision at watch creation (`view.ts:280-285`).
    pub revision: i64,
    on_report: Arc<dyn Fn(&anyhow::Error) + Send + Sync>,
    /// Upstream's `onStop` closure (`view.ts:63-67`): detaches the watcher
    /// from its record and evicts the record when the last watcher stops.
    /// Run once, from [`Watch::stop`].
    on_stop: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

/// A delivered-envelope listener (`view.ts:35`).
type EnvelopeListener = Arc<dyn Fn(&Envelope) + Send + Sync>;

struct WatchState {
    listener: Option<EnvelopeListener>,
    buffer: Vec<Envelope>,
    stopped: bool,
}

impl Watch {
    /// Upstream `start` (`view.ts:287-294`): idempotent; replays the
    /// pre-start buffer in order.
    pub fn start(&self, listener: EnvelopeListener) {
        let mut state = state_lock(&self.state);
        if state.listener.is_some() || state.stopped {
            return;
        }
        state.listener = Some(listener);
        let buffered: Vec<Envelope> = std::mem::take(&mut state.buffer);
        let listener = state.listener.clone().expect("just set");
        drop(state);
        for envelope in buffered {
            if self.is_closed() {
                break;
            }
            self.deliver(&listener, envelope);
        }
    }

    /// Upstream `stop` (`view.ts:296-301`): idempotent; a hard no-callback
    /// boundary. Stopping also detaches the watcher from its manager record
    /// and evicts the record when the last watcher stopped
    /// (`view.ts:63-67`).
    pub fn stop(&self) {
        let on_stop = {
            let mut state = state_lock(&self.state);
            if state.stopped {
                return;
            }
            state.stopped = true;
            state.buffer.clear();
            state.listener = None;
            self.on_stop.lock().expect("on stop").take()
        };
        if let Some(on_stop) = on_stop {
            on_stop();
        }
    }

    /// Upstream `get closed` (`view.ts:283-285`).
    pub fn is_closed(&self) -> bool {
        state_lock(&self.state).stopped
    }

    /// Upstream `accept` (`view.ts:303-315`).
    fn accept(&self, envelope: Envelope) {
        let mut state = state_lock(&self.state);
        if state.stopped {
            return;
        }
        if state.listener.is_none() {
            if state.buffer.len() >= WATCH_CAPACITY {
                drop(state);
                self.stop();
                (self.on_report)(&anyhow::Error::msg(format!(
                    "watch capacity {WATCH_CAPACITY} exceeded before start()"
                )));
                return;
            }
            state.buffer.push(envelope);
            return;
        }
        let listener = state.listener.clone().expect("checked above");
        drop(state);
        self.deliver(&listener, envelope);
    }

    /// Upstream `deliver` (`view.ts:317-323`): a throwing listener fails
    /// this watch and reports; the panic is caught, not propagated.
    fn deliver(&self, listener: &EnvelopeListener, envelope: Envelope) {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            listener(&envelope);
        }));
        if let Err(payload) = outcome {
            let message = if let Some(text) = payload.downcast_ref::<&str>() {
                (*text).to_owned()
            } else if let Some(text) = payload.downcast_ref::<String>() {
                text.clone()
            } else {
                "watch listener panicked".to_owned()
            };
            self.stop();
            (self.on_report)(&anyhow::Error::msg(message));
        }
    }

    /// Upstream `fail` (`view.ts:325-328`).
    fn fail(&self) {
        self.stop();
    }
}

fn state_lock(state: &Mutex<WatchState>) -> std::sync::MutexGuard<'_, WatchState> {
    state.lock().expect("watch state")
}

/// Upstream `applyEnvelope` (`view.ts:31-33`).
pub fn apply_envelope(
    view: &ConversationView,
    envelope: &Envelope,
) -> anyhow::Result<ConversationView> {
    let current = serde_json::to_value(view)?;
    let applied = apply_immutable(Some(&current), &envelope.ops)?;
    Ok(serde_json::from_value(applied)?)
}

struct ViewRecord {
    tracker: Tracker,
    watchers: Vec<Arc<Watch>>,
    revision: i64,
}

/// Upstream `ViewManager` (`view.ts:44-260`).
pub struct ViewManager {
    inner: Arc<ManagerInner>,
}

struct ManagerInner {
    records: Mutex<HashMap<Id, ViewRecord>>,
    deliveries: Mutex<Vec<(Envelope, Vec<Arc<Watch>>)>>,
    session: Arc<Session>,
}

impl Clone for ViewManager {
    fn clone(&self) -> ViewManager {
        ViewManager {
            inner: self.inner.clone(),
        }
    }
}

impl ViewManager {
    /// Upstream `new ViewManager(session, onReport)` (`view.ts:50-53`):
    /// listener failures report through the session's `onReport`.
    pub fn new(session: Arc<Session>) -> Arc<ViewManager> {
        Arc::new(ViewManager {
            inner: Arc::new(ManagerInner {
                records: Mutex::new(HashMap::new()),
                deliveries: Mutex::new(Vec::new()),
                session,
            }),
        })
    }

    /// An owned handle for the onStop closure (`view.ts:63-67`).
    fn self_clone(&self) -> ViewManager {
        self.clone()
    }

    /// Upstream `watch` (`view.ts:55-70`): build or reuse the conversation
    /// record; watchers see the snapshot as of creation.
    pub fn watch(
        &self,
        conversation: &Conversation,
        entries: Vec<Entry>,
    ) -> anyhow::Result<Arc<Watch>> {
        self.watch_using(conversation, || self.build(conversation, &entries))
    }

    /// Capture and subscribe while the Session line is held. Preloaded trackers
    /// belong to Tx until commit returns, so the shared cache is not readable here.
    pub(crate) fn watch_in_tx(
        &self,
        conversation: &Conversation,
        entries: Vec<Entry>,
        tx: &mut super::session::Tx,
    ) -> anyhow::Result<Arc<Watch>> {
        self.watch_using(conversation, || {
            let rewindable = tx.snapshot(super::types::DocRef::Rewindable {
                conversation_id: conversation.id,
            })?;
            let sticky = tx.snapshot(super::types::DocRef::Sticky {
                conversation_id: conversation.id,
            })?;
            let session = tx.snapshot(super::types::DocRef::Session)?;
            self.build_with_docs(conversation, &entries, &rewindable, &sticky, &session)
        })
    }

    fn watch_using(
        &self,
        conversation: &Conversation,
        build: impl FnOnce() -> anyhow::Result<ConversationView>,
    ) -> anyhow::Result<Arc<Watch>> {
        let mut records = self.inner.records.lock().expect("view records");
        let record = match records.get_mut(&conversation.id) {
            Some(record) => record,
            None => {
                let view = build()?;
                let mut tracker = track(serde_json::to_value(&view)?);
                // Consume the synthetic base flush (`view.ts:59`).
                let _ = tracker.flush();
                records.insert(
                    conversation.id,
                    ViewRecord {
                        tracker,
                        watchers: Vec::new(),
                        revision: 0,
                    },
                );
                records.get_mut(&conversation.id).expect("just inserted")
            }
        };
        let snapshot: ConversationView = serde_json::from_value(record.tracker.state().clone())?;
        let session = self.inner.session.clone();
        let watch = Arc::new(Watch {
            state: Arc::new(Mutex::new(WatchState {
                listener: None,
                buffer: Vec::new(),
                stopped: false,
            })),
            view: snapshot.clone(),
            revision: record.revision,
            on_report: Arc::new(move |error: &anyhow::Error| {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    session.report_public(error);
                }));
            }),
            on_stop: Mutex::new(None),
        });
        record.watchers.push(watch.clone());
        // Upstream's onStop closure (`view.ts:63-67`): on stop, remove this
        // watcher from the record; when the record's last watcher is gone,
        // drop the record so a later watch rebuilds fresh.
        let manager = self.self_clone();
        let weak = Arc::downgrade(&watch);
        let conversation_id = conversation.id;
        *watch.on_stop.lock().expect("on stop") = Some(Box::new(move || {
            if let Some(watch) = weak.upgrade() {
                manager.detach_watcher(conversation_id, &watch);
            }
        }));
        Ok(watch)
    }

    /// Runs on the Session line after persistence and in-memory indexes
    /// update (`view.ts:72-133`).
    pub fn update(&self, result: &CommitRecord) {
        // (watcher, report) pairs: watcher stops re-enter the manager, so
        // they run after the records lock is released.
        let mut failed: Vec<(Arc<Watch>, Option<String>)> = Vec::new();
        {
            let mut records = self.inner.records.lock().expect("view records");
            let conversation_ids: Vec<Id> = records.keys().copied().collect();
            for conversation_id in conversation_ids {
                let conversation = match self
                    .inner
                    .session
                    .conversation_records()
                    .get(&conversation_id)
                {
                    Some(conversation) => conversation.clone(),
                    None => continue,
                };
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    self.update_record(&mut records, conversation_id, &conversation, result)
                }));
                match outcome {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        // The record's watchers fail and the record is
                        // dropped (`view.ts:128-131`).
                        if let Some(record) = records.remove(&conversation_id) {
                            failed.extend(
                                record
                                    .watchers
                                    .into_iter()
                                    .map(|watcher| (watcher, Some(format!("{error}")))),
                            );
                        }
                    }
                    Err(payload) => {
                        if let Some(record) = records.remove(&conversation_id) {
                            failed.extend(record.watchers.into_iter().map(|watcher| {
                                (watcher, Some(format!("view update panicked: {payload:?}")))
                            }));
                        }
                    }
                }
            }
        }
        for (watcher, report) in failed {
            watcher.fail();
            if let Some(report) = report {
                self.report_error(anyhow::Error::msg(report));
            }
        }
    }

    /// Upstream's onStop body (`view.ts:63-67`): remove the watcher from its
    /// record; evict the record when its last watcher stopped.
    fn detach_watcher(&self, conversation_id: Id, watcher: &Arc<Watch>) {
        let mut records = self.inner.records.lock().expect("view records");
        let Some(record) = records.get_mut(&conversation_id) else {
            return;
        };
        record
            .watchers
            .retain(|existing| !Arc::ptr_eq(existing, watcher));
        if record.watchers.is_empty() {
            records.remove(&conversation_id);
        }
    }

    /// Test visibility: live watcher counts per conversation record.
    #[cfg(test)]
    pub(crate) fn watcher_counts(&self) -> Vec<(Id, usize)> {
        self.inner
            .records
            .lock()
            .expect("view records")
            .iter()
            .map(|(id, record)| (*id, record.watchers.len()))
            .collect()
    }

    fn update_record(
        &self,
        records: &mut HashMap<Id, ViewRecord>,
        conversation_id: Id,
        conversation: &Conversation,
        result: &CommitRecord,
    ) -> anyhow::Result<()> {
        let changes = &result.changes;
        let appended: Vec<&Entry> = changes
            .entries
            .iter()
            .filter(|entry| entry.conversation_id == conversation_id)
            .collect();
        let document_changes: Vec<&(super::types::DocRef, Vec<Op>)> = changes
            .docs
            .iter()
            .filter(|(reference, _)| match reference {
                super::types::DocRef::Session => true,
                super::types::DocRef::Rewindable {
                    conversation_id: owner,
                }
                | super::types::DocRef::Sticky {
                    conversation_id: owner,
                } => *owner == conversation_id,
            })
            .collect();
        let task_changed = changes
            .tasks
            .iter()
            .any(|task| task.conversation_id == conversation_id);
        let events: Vec<&ViewEvent> = changes
            .events
            .iter()
            .filter(|(owner, _)| *owner == conversation_id)
            .map(|(_, event)| event)
            .collect();
        if appended.is_empty() && document_changes.is_empty() && !task_changed && events.is_empty()
        {
            return Ok(());
        }

        // Rebuild only the affected projections and diff through the tracker
        // (`view.ts:89-118`).
        let rewindable_changed = document_changes.iter().any(|(reference, _)| {
            matches!(reference, super::types::DocRef::Rewindable { conversation_id: owner } if *owner == conversation_id)
        });
        let sticky_changed = document_changes.iter().any(|(reference, _)| {
            matches!(reference, super::types::DocRef::Sticky { conversation_id: owner } if *owner == conversation_id)
        });
        let config_changed = events
            .iter()
            .any(|event| event.event_type() == "config.changed");
        let plugin_changed = document_changes
            .iter()
            .any(|(_, ops)| touches_key(ops, "plugins"));
        let current_state: ConversationView = serde_json::from_value(
            records
                .get(&conversation_id)
                .expect("record")
                .tracker
                .state()
                .clone(),
        )?;
        let rewindable = if rewindable_changed || config_changed || plugin_changed {
            Some(self.document_json(&super::types::DocRef::Rewindable { conversation_id })?)
        } else {
            None
        };
        let sticky = if sticky_changed || task_changed || config_changed || plugin_changed {
            Some(self.document_json(&super::types::DocRef::Sticky { conversation_id })?)
        } else {
            None
        };
        let session_doc = if plugin_changed {
            Some(self.document_json(&super::types::DocRef::Session)?)
        } else {
            None
        };

        let mut next = current_state.clone();
        if config_changed {
            next.config = self.config(
                rewindable.as_ref().expect("loaded for config"),
                sticky.as_ref().expect("loaded for config"),
            )?;
        }
        if sticky_changed {
            next.inbox = sticky_inbox(sticky.as_ref().expect("loaded for inbox"))?;
        }
        if sticky_changed || task_changed {
            next.turn = self.turn(conversation_id, sticky.as_ref().expect("loaded for turn"))?;
            next.compaction = self.compaction(conversation_id);
            next.tasks = self.tasks(conversation_id, sticky.as_ref().expect("loaded for tasks"))?;
        }
        if plugin_changed {
            next.plugins = self.plugins(
                rewindable.as_ref().expect("loaded for plugins"),
                sticky.as_ref().expect("loaded for plugins"),
                &session_doc.expect("loaded for plugins"),
            )?;
        }
        // Entries: transcript splices for head commits, then appends
        // (`applyEntries`, `view.ts:337-354`), folded into the same value so
        // the tracker emits the entry ops plus projection ops.
        let mut entries_json: Vec<Value> = {
            let state = records
                .get(&conversation_id)
                .expect("record")
                .tracker
                .state()
                .clone();
            state
                .get("entries")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        };
        let mut entry_ops: Vec<Op> = Vec::new();
        for entry in &appended {
            if let Some(head) = entry.head {
                let retained = entries_json
                    .iter()
                    .position(|candidate| {
                        candidate
                            .get("id")
                            .and_then(Value::as_i64)
                            .is_some_and(|id| id >= head)
                    })
                    .unwrap_or(entries_json.len());
                if retained > 0 {
                    entries_json.drain(0..retained);
                    entry_ops.push(Op::Splice {
                        path: vec![segment_key("entries")],
                        index: 0,
                        remove: retained,
                        items: Vec::new(),
                    });
                }
            }
            entries_json.push(serde_json::to_value(entry)?);
        }
        let mut next_value = serde_json::to_value(&next)?;
        if !appended.is_empty() {
            if let Some(object) = next_value.as_object_mut() {
                object.insert("entries".to_owned(), Value::Array(entries_json));
            }
        }
        let record = records.get_mut(&conversation_id).expect("record");
        {
            let tracker_state = record.tracker.state_mut();
            *tracker_state = next_value;
        }
        let mut ops: Vec<Op> = entry_ops;
        ops.extend(record.tracker.flush());
        if ops.is_empty() && events.is_empty() {
            return Ok(());
        }
        record.revision += 1;
        let envelope = Envelope {
            revision: record.revision,
            ops,
            events: events.into_iter().cloned().collect(),
        };
        let watchers = record.watchers.clone();
        let _ = conversation;
        self.inner
            .deliveries
            .lock()
            .expect("deliveries")
            .push((envelope, watchers));
        Ok(())
    }

    /// Runs after the Session line; listeners are synchronous and ordered
    /// (`view.ts:135-140`).
    pub fn deliver(&self) {
        let deliveries: Vec<(Envelope, Vec<Arc<Watch>>)> = self
            .inner
            .deliveries
            .lock()
            .expect("deliveries")
            .drain(..)
            .collect();
        for (envelope, watchers) in deliveries {
            for watcher in watchers {
                watcher.accept(envelope.clone());
            }
        }
    }

    /// Upstream `close` (`view.ts:142-147`).
    pub fn close(&self) {
        let watchers: Vec<Arc<Watch>> = {
            let mut records = self.inner.records.lock().expect("view records");
            let watchers: Vec<Arc<Watch>> = records
                .drain()
                .flat_map(|(_, record)| record.watchers)
                .collect();
            self.inner.deliveries.lock().expect("deliveries").clear();
            watchers
        };
        // Watcher stops re-enter detach_watcher, so the records lock must
        // be released first.
        for watcher in watchers {
            watcher.stop();
        }
    }

    /// Upstream `build` (`view.ts:149-164`): the full snapshot for a fresh
    /// conversation record.
    fn build(
        &self,
        conversation: &Conversation,
        entries: &[Entry],
    ) -> anyhow::Result<ConversationView> {
        let rewindable = self.document_json(&super::types::DocRef::Rewindable {
            conversation_id: conversation.id,
        })?;
        let sticky = self.document_json(&super::types::DocRef::Sticky {
            conversation_id: conversation.id,
        })?;
        let session = self.document_json(&super::types::DocRef::Session)?;
        self.build_with_docs(conversation, entries, &rewindable, &sticky, &session)
    }

    fn build_with_docs(
        &self,
        conversation: &Conversation,
        entries: &[Entry],
        rewindable: &JsonObject,
        sticky: &JsonObject,
        session: &JsonObject,
    ) -> anyhow::Result<ConversationView> {
        // `Omit<Conversation, "sections">` (`view.ts:153`).
        let mut public_conversation = serde_json::to_value(conversation)?;
        if let Some(object) = public_conversation.as_object_mut() {
            object.shift_remove("sections");
        }
        let mut view = ConversationView {
            conversation: public_conversation,
            entries: entries.to_vec(),
            config: self.config(rewindable, sticky)?,
            inbox: sticky_inbox(sticky)?,
            turn: None,
            compaction: None,
            tasks: self.tasks(conversation.id, sticky)?,
            plugins: self.plugins(rewindable, sticky, session)?,
        };
        view.turn = self.turn(conversation.id, sticky)?;
        view.compaction = self.compaction(conversation.id);
        Ok(view)
    }

    /// Upstream `document` (`view.ts:166-170`): a loaded document from the
    /// session cache.
    fn document_json(&self, reference: &super::types::DocRef) -> anyhow::Result<JsonObject> {
        match self.inner.session.loaded_document(reference) {
            Some(document) => Ok(document),
            None => anyhow::bail!("view document {} is not loaded", doc_label(reference)),
        }
    }

    /// Upstream `config` (`view.ts:172-180`): every declared key's effective
    /// value across both documents.
    fn config(&self, rewindable: &JsonObject, sticky: &JsonObject) -> anyhow::Result<JsonObject> {
        let defaults = self.inner.session.defaults();
        let mut out = JsonObject::new();
        for (key, doc) in &defaults.route {
            let source = if doc == "rewindable" {
                rewindable
            } else {
                sticky
            };
            let fallback = if doc == "rewindable" {
                defaults.rewindable.get(key)
            } else {
                defaults.sticky.get(key)
            };
            // `value !== undefined` (`view.ts:178`): a stored or declared
            // null is a value and renders; only absence is skipped.
            if let Some(value) = source.get(key).or(fallback) {
                out.insert(key.clone(), value.clone());
            }
        }
        Ok(out)
    }

    /// Upstream `turn` (`view.ts:182-199`).
    fn turn(&self, conversation_id: Id, sticky: &JsonObject) -> anyhow::Result<Option<TurnView>> {
        let kinds = self.inner.session.kinds();
        let kinds = kinds.read().expect("kind registry");
        let live_tasks = self.inner.session.live_tasks();
        let mut live: Vec<&Task> = live_tasks
            .values()
            .filter(|task| task.conversation_id == conversation_id)
            .collect();
        live.sort_by_key(|task| task.id);
        let turn_tasks: Vec<&Task> = live
            .iter()
            .copied()
            .filter(|task| {
                kinds
                    .get(&task.kind)
                    .map(|kind: &Arc<dyn super::types::AnyKind>| kind.turn())
                    .unwrap_or(false)
            })
            .collect();
        if turn_tasks.is_empty() {
            return Ok(None);
        }
        let generation = turn_tasks
            .iter()
            .find(|task| task.kind == "pi.generation")
            .copied();
        let post_tools = turn_tasks
            .iter()
            .find(|task| task.kind == "pi.post_tools")
            .copied();
        let input_task = generation.or(post_tools);
        let inputs: Vec<Id> = match input_task {
            Some(task) => input_ids(task),
            None => Vec::new(),
        };
        let streaming = sticky
            .get("turn")
            .and_then(|turn| turn.get("message"))
            .map(|message| !message.is_null())
            .unwrap_or(false);
        let message = sticky
            .get("turn")
            .and_then(|turn| turn.get("message"))
            .filter(|message| !message.is_null())
            .cloned();
        let tools = sticky
            .get("turn")
            .and_then(|turn| turn.get("tools"))
            .and_then(Value::as_array)
            .map(|tools| tools.iter().map(strip_private_tool_state).collect())
            .unwrap_or_default();
        Ok(Some(TurnView {
            inputs,
            generation: generation.map(|task| generation_status(task, streaming)),
            message,
            tools,
        }))
    }

    /// Upstream `compaction` (`view.ts:201-219`).
    fn compaction(&self, conversation_id: Id) -> Option<super::types::CompactionView> {
        let live_tasks = self.inner.session.live_tasks();
        let mut live: Vec<&Task> = live_tasks
            .values()
            .filter(|task| task.conversation_id == conversation_id && task.kind == "pi.collapse")
            .collect();
        live.sort_by_key(|task| task.id);
        let task = live.into_iter().next()?;
        let reason = task
            .input
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let checkpoint = task.checkpoint.as_ref();
        let phase = checkpoint.and_then(|c| checkpoint_phase(c));
        let attempt = checkpoint
            .and_then(|c| c.get("attempt"))
            .and_then(Value::as_i64)
            .unwrap_or(1);
        let until_ms = checkpoint
            .and_then(|c| c.get("untilMs"))
            .and_then(Value::as_i64);
        Some(super::types::CompactionView {
            task_id: task.id,
            reason,
            stage: if phase == Some("retrying") {
                "retrying"
            } else {
                "summarizing"
            }
            .to_owned(),
            attempt,
            retry_at: if phase == Some("retrying") {
                until_ms
            } else {
                None
            },
        })
    }

    /// Upstream `tasks` (`view.ts:221-241`).
    fn tasks(
        &self,
        conversation_id: Id,
        sticky: &JsonObject,
    ) -> anyhow::Result<HashMap<String, super::types::TaskViewSummary>> {
        let mut out = HashMap::new();
        let kinds = self.inner.session.kinds();
        let kinds = kinds.read().expect("kind registry");
        let live_tasks = self.inner.session.live_tasks();
        let mut live: Vec<&Task> = live_tasks
            .values()
            .filter(|task| task.conversation_id == conversation_id)
            .collect();
        live.sort_by_key(|task| task.id);
        for task in live {
            let Some(kind) = kinds.get(&task.kind) else {
                continue;
            };
            if kind.turn() || task.kind == "pi.collapse" {
                continue;
            }
            let slot = sticky
                .get("tasks")
                .and_then(|tasks| tasks.get(task.id.to_string()))
                .and_then(Value::as_object)
                .cloned()
                .map(|mut slot| {
                    // Private coordination memos are never passed to user-defined
                    // describe callbacks, not merely removed from their output.
                    slot.shift_remove("memos");
                    slot
                });
            out.insert(
                task.id.to_string(),
                super::types::TaskViewSummary {
                    kind: task.kind.clone(),
                    background: task.background,
                    marked: if task.abort == Some(true) {
                        Some(true)
                    } else {
                        None
                    },
                    status: strict_json(
                        &kind.describe(task, slot.as_ref())?,
                        &format!("task kind {}", task.kind),
                    )?,
                },
            );
        }
        Ok(out)
    }

    /// Upstream `plugins` (`view.ts:243-259`): the registered projections
    /// over the merged namespace slices.
    fn plugins(
        &self,
        rewindable: &JsonObject,
        sticky: &JsonObject,
        session: &JsonObject,
    ) -> anyhow::Result<JsonObject> {
        let mut out = JsonObject::new();
        let namespaces = self.inner.session.namespaces().read().expect("namespaces");
        for (id, registration) in namespaces.iter() {
            let Some(project) = &registration.project else {
                continue;
            };
            let mut merged = JsonObject::new();
            for source in [
                &registration.defaults.rewindable,
                &registration.defaults.sticky,
                &registration.defaults.session,
            ] {
                for (key, value) in source {
                    merged.insert(key.clone(), value.clone());
                }
            }
            for document in [rewindable, sticky] {
                if let Some(slice) = document
                    .get("plugins")
                    .and_then(|plugins| plugins.get(id))
                    .and_then(Value::as_object)
                {
                    for (key, value) in slice {
                        merged.insert(key.clone(), value.clone());
                    }
                }
            }
            if let Some(slice) = session
                .get("plugins")
                .and_then(|plugins| plugins.get(id))
                .and_then(Value::as_object)
            {
                for (key, value) in slice {
                    merged.insert(key.clone(), value.clone());
                }
            }
            let projected = project(&merged)?;
            out.insert(
                id.clone(),
                strict_json(&projected, &format!("namespace {id} view"))?,
            );
        }
        Ok(out)
    }

    fn report_error(&self, error: anyhow::Error) {
        // `onReport` failures are swallowed (`view.ts:330-334`).
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.inner.session.report_public(&error);
        }));
    }
}

fn doc_label(reference: &super::types::DocRef) -> &'static str {
    match reference {
        super::types::DocRef::Session => "session",
        super::types::DocRef::Rewindable { .. } => "rewindable",
        super::types::DocRef::Sticky { .. } => "sticky",
    }
}

fn segment_key(key: &str) -> crate::agent_core::chord_support::delta::Seg {
    crate::agent_core::chord_support::delta::Seg::Key(key.to_owned())
}

/// Upstream `inputIds` (`view.ts:356-359`).
fn input_ids(task: &Task) -> Vec<Id> {
    task.input
        .get("inputs")
        .and_then(Value::as_array)
        .map(|inputs| inputs.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default()
}

/// Upstream `generationStatus` (`view.ts:361-384`).
fn generation_status(task: &Task, streaming: bool) -> GenerationStatus {
    if task.status == TaskStatus::Pending && !task.after.is_empty() {
        return GenerationStatus::Waiting {
            on: "compaction".to_owned(),
        };
    }
    let checkpoint = task.checkpoint.as_ref();
    let phase = checkpoint.and_then(|checkpoint| checkpoint_phase(checkpoint));
    let attempt = || {
        checkpoint
            .and_then(|checkpoint| checkpoint.get("attempt"))
            .and_then(Value::as_i64)
            .unwrap_or(1)
    };
    match phase {
        Some("requesting") => {
            GenerationStatus::Streaming { attempt: attempt() }.promote_if(streaming, attempt())
        }
        Some("retrying") => GenerationStatus::Retrying {
            attempt: attempt(),
            retry_at: checkpoint
                .and_then(|checkpoint| checkpoint.get("untilMs"))
                .and_then(Value::as_i64)
                .unwrap_or(0),
            last_error: checkpoint
                .and_then(|checkpoint| checkpoint.get("lastError"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        },
        Some("deferred") => GenerationStatus::Deferred {
            attempt: attempt(),
            poll_at: checkpoint
                .and_then(|checkpoint| checkpoint.get("pollAt"))
                .and_then(Value::as_i64)
                .unwrap_or(0),
        },
        _ => GenerationStatus::Preparing,
    }
}

impl GenerationStatus {
    fn promote_if(self, streaming: bool, attempt: i64) -> GenerationStatus {
        match (self, streaming) {
            (GenerationStatus::Streaming { .. }, false) => GenerationStatus::Requesting { attempt },
            (status, _) => status,
        }
    }
}

/// Upstream `stripPrivateToolState` (`view.ts:386-389`).
fn strip_private_tool_state(slot: &Value) -> Value {
    let mut slot = slot.clone();
    if let Some(object) = slot.as_object_mut() {
        object.shift_remove("memos");
    }
    slot
}

/// Upstream `strictJson` (`view.ts:403-407`).
fn strict_json(value: &Value, what: &str) -> anyhow::Result<Value> {
    match serde_json::to_vec(value) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(_) => anyhow::bail!("{what} returned a non-JSON value"),
    }
}

/// Upstream `touchesKey` (`view.ts:409-411`).
fn touches_key(ops: &[Op], key: &str) -> bool {
    ops.iter().any(|op| match op {
        Op::Replace(_) => true,
        other => other
            .path()
            .and_then(|path| path.first())
            .map(|segment| {
                matches!(segment, crate::agent_core::chord_support::delta::Seg::Key(first) if first == key)
            })
            .unwrap_or(false),
    })
}

/// The sticky inbox as typed queued inputs (`view.ts:109`).
fn sticky_inbox(sticky: &JsonObject) -> anyhow::Result<Vec<super::types::QueuedInput>> {
    let inbox = sticky
        .get("inbox")
        .cloned()
        .unwrap_or(Value::Array(Vec::new()));
    Ok(serde_json::from_value(inbox)?)
}

/// Whether the sticky document records a live streaming message (`view.ts:194`).
#[allow(dead_code)]
fn sticky_streaming(sticky: &JsonObject) -> bool {
    sticky
        .get("turn")
        .and_then(|turn| turn.get("message"))
        .map(|message| !message.is_null())
        .unwrap_or(false)
}

#[cfg(test)]
mod json_order_tests {
    use super::*;

    #[test]
    fn json_order_private_memos_do_not_reorder_public_tool_state() {
        let slot = serde_json::json!({"memos":{"x":1},"z":2,"a":3,"b":4});
        let before = slot.to_string();
        assert_eq!(
            strip_private_tool_state(&slot).to_string(),
            r#"{"z":2,"a":3,"b":4}"#
        );
        assert_eq!(slot.to_string(), before);
    }
}
