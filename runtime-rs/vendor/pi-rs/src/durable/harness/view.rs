//! Port of `src/harness/view.ts`: the conversation view mounts of one
//! Harness — at most one mount per conversation, built on the Session line by
//! its first observer and dropped with its last. Each mount advances from the
//! Session's commit publications, which are durable (spec §9.3).
//!
//! Divergences (structural, disclosed):
//! - **D24 (detached chord state).** `ConversationViews.state()` and
//!   `Conversation.viewState()` resolve over the chord
//!   `replicatedState(source)` constructor
//!   ([`crate::chord::api::replicated_state_from_source`]) with the
//!   Session's [`CommittedStateSource`] as the authoritative source — the
//!   same wiring as `Session.documentState()` (D9). The state value is the
//!   strict-JSON view object.
//! - **D25 (view value).** The mount value is the strict-JSON object
//!   `{conversation, entries, docs}` in the upstream construction order; the
//!   port exposes it as [`ConversationView`] over `serde_json` maps, applies
//!   the mount's derived operations with the chord `applyImmutable`, and
//!   delivers the same `["docs", kind]` / `["entries"]` op paths.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use serde_json::{Map, Value};

/// One mounted document kind's `(record_id, version)` incarnation bounds
/// (`#build`'s `incarnations` map).
type MountedIncarnations = HashMap<String, (i64, i64)>;

/// `attach_hydrate`'s result: the mount's revision plus, for a freshly built
/// mount, the revision to install and the mounted incarnations.
type HydratedMount = (Value, Option<(Value, MountedIncarnations)>);

use crate::agent_core::chord_support::context::{abort_signal_key, Context};
use crate::chord::delta::{apply_immutable, Op, Seg};
use crate::chord::services::state::AttachedReplicatedState;

use super::super::errors::PlainError;
use super::super::harness::config::conversation_config;
use super::super::harness::context::{active_entries, capture_context_bounds};
use super::super::harness::inbox::inbox_doc;
use super::super::harness::live::live_doc;
use super::super::harness::usage::usage_doc;
use super::super::ids::ConversationId;
use super::super::session::observation::{CommittedStateSource, CommittedWatch};
use super::super::session::session::Session;
use super::super::storage::Storage;
use super::super::types::{
    CommitChange, CommitPublication, DocumentCommitChange, EntryRecord, JsonObject, WatchEnd,
};

/// Structural mount of one conversation's active transcript and built-in
/// documents (`ConversationView`): `{conversation, entries, docs}`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ConversationView {
    pub conversation: Option<JsonObject>,
    /// Raw active entries: the head marker, then the non-head entries from
    /// its head.
    pub entries: Vec<EntryRecord>,
    /// Built-in conversation documents keyed by kind (upstream: "`pi.agent`,
    /// `pi.live`, `pi.inbox`, `pi.usage`, and the fresh `pi.provider`, keyed
    /// by kind"); absent documents are absent. The port mounts
    /// `pi.conversation.config` for `pi.agent`, per the base-architecture
    /// ruling recorded on the module.
    pub docs: Map<String, Value>,
}

impl ConversationView {
    /// The strict-JSON view value in the upstream construction order.
    pub fn to_json(&self) -> Value {
        let mut value = Map::new();
        value.insert(
            String::from("conversation"),
            self.conversation
                .clone()
                .map(Value::Object)
                .unwrap_or(Value::Null),
        );
        value.insert(
            String::from("entries"),
            Value::Array(
                self.entries
                    .iter()
                    .map(|entry| serde_json::to_value(entry).unwrap_or(Value::Null))
                    .collect(),
            ),
        );
        value.insert(String::from("docs"), Value::Object(self.docs.clone()));
        Value::Object(value)
    }

    /// Parse a view value.
    pub fn from_json(value: &Value) -> ConversationView {
        let object = value.as_object().cloned().unwrap_or_default();
        ConversationView {
            conversation: object
                .get("conversation")
                .and_then(Value::as_object)
                .cloned(),
            entries: object
                .get("entries")
                .and_then(Value::as_array)
                .map(|entries| {
                    entries
                        .iter()
                        .filter_map(|entry| serde_json::from_value(entry.clone()).ok())
                        .collect()
                })
                .unwrap_or_default(),
            docs: object
                .get("docs")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
        }
    }
}

/// One observer of a mount: every next revision and the Session's close
/// (`ViewObserver`).
pub trait ViewObserver: Send + Sync {
    /// The mount's next revision with the operations that produced it.
    fn advance(&self, value: &Value, ops: &[Op], context: &Context);
    /// Every publication, after the mount took it; `ops` are the mount's,
    /// possibly none.
    fn publication(
        &self,
        before: &Value,
        after: &Value,
        ops: &[Op],
        publication: &CommitPublication,
        context: &Context,
    );
    /// The Session closed.
    fn close_session(&self);
}

/// The built-in mounted document kinds (`MOUNTED`): the conversation's agent
/// document (`pi.agent`, the port's `pi.conversation.config`), `pi.live`,
/// `pi.inbox`, the fresh `pi.provider`, and `pi.usage`.
fn mounted_kinds() -> BTreeSet<&'static str> {
    BTreeSet::from([
        "pi.conversation.config",
        "pi.live",
        "pi.inbox",
        "pi.provider",
        "pi.usage",
    ])
}

/// One conversation's mount (`Mount`).
struct Mount {
    value: Value,
    /// Mounted incarnation and definition version per kind; another
    /// incarnation or version is set whole.
    docs: HashMap<String, (i64, i64)>,
    observers: Vec<Arc<dyn ViewObserver>>,
}

#[derive(Default)]
struct ViewsCore {
    mounts: HashMap<ConversationId, Mount>,
}

/// The Harness's conversation view mounts (`ConversationViews`).
pub struct ConversationViews {
    session: Arc<Session>,
    storage: Arc<dyn Storage>,
    core: Mutex<ViewsCore>,
    closed: Mutex<bool>,
    self_ref: OnceLock<Weak<ConversationViews>>,
}

impl ConversationViews {
    /// Build the views and subscribe to the Session's publications and close
    /// (`constructor`).
    pub fn new(session: Arc<Session>, storage: Arc<dyn Storage>) -> Result<Arc<Self>, PlainError> {
        let views = Arc::new(ConversationViews {
            session: Arc::clone(&session),
            storage,
            core: Mutex::new(ViewsCore::default()),
            closed: Mutex::new(false),
            self_ref: OnceLock::new(),
        });
        let _ = views.self_ref.set(Arc::downgrade(&views));
        let observer = Arc::downgrade(&views);
        session.subscribe_commits(Arc::new(move |publication, context| {
            if let Some(views) = observer.upgrade() {
                let ids: Vec<ConversationId> = views
                    .core
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .mounts
                    .keys()
                    .copied()
                    .collect();
                for id in ids {
                    views.advance(id, publication, context);
                }
            }
        }))?;
        let closer = Arc::downgrade(&views);
        session.subscribe_close(Arc::new(move || {
            if let Some(views) = closer.upgrade() {
                *views
                    .closed
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
                let mut core = views
                    .core
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                for mount in core.mounts.values() {
                    for observer in mount.observers.clone() {
                        observer.close_session();
                    }
                }
                core.mounts.clear();
            }
        }))?;
        Ok(views)
    }

    /// A disposable read-only Chord state of the view (`state`): the mount's
    /// current revision published through a [`CommittedStateSource`], on the
    /// Session line like every attach.
    pub async fn state(
        self: &Arc<Self>,
        id: ConversationId,
        context: Context,
    ) -> Result<Arc<AttachedReplicatedState>, PlainError> {
        let views = Arc::clone(self);
        self.session
            .read_on_line(|| {
                let views = Arc::clone(&views);
                let context = context.clone();
                async move {
                    let (value, fresh) = views.attach_hydrate(id, &context)?;
                    // `this.attach(id, (value, release) => new
                    // CommittedStateSource<ConversationView>(value, release),
                    // context)`: the source's release drops the observer and,
                    // with the last observer, its mount.
                    let (slot, release) = mount_release(&views, id);
                    let source = CommittedStateSource::new(value, release);
                    let observer: Arc<dyn ViewObserver> =
                        Arc::clone(&source) as Arc<dyn ViewObserver>;
                    install_observer(&slot, &observer);
                    views.attach_register(id, &observer, &context, fresh)?;
                    // `replicatedState(observer)`; a construction failure
                    // detaches before propagating.
                    match crate::chord::api::replicated_state_from_source(
                        Arc::clone(&source) as Arc<dyn crate::chord::types::ReplicatedStateSource>,
                        crate::chord::types::ReplicatedStateSourceOptions::default(),
                    ) {
                        Ok(state) => Ok(state),
                        Err(error) => {
                            views.detach(id, &observer);
                            Err(PlainError::new(error.message()))
                        }
                    }
                }
            })
            .await
    }

    /// A serialized exact-frame watch of the view; cancelling `context`
    /// stops it (`watch`).
    pub async fn watch(
        self: &Arc<Self>,
        id: ConversationId,
        context: Context,
    ) -> Result<Arc<CommittedWatch>, PlainError> {
        let views = Arc::clone(self);
        let attached: Result<Arc<CommittedWatch>, PlainError> = self
            .session
            .read_on_line(|| {
                let views = Arc::clone(&views);
                let context = context.clone();
                async move {
                    let (value, fresh) = views.attach_hydrate(id, &context)?;
                    let (slot, release) = mount_release(&views, id);
                    let watch = Arc::new(CommittedWatch::new(value, Box::new(release), None));
                    let observer: Arc<dyn ViewObserver> = Arc::new(WatchObserver {
                        watch: Arc::clone(&watch),
                    });
                    install_observer(&slot, &observer);
                    views.attach_register(id, &observer, &context, fresh)?;
                    Ok(watch)
                }
            })
            .await;
        let watch: Arc<CommittedWatch> = attached?;
        let signal = context.abort_signal();
        if signal.as_ref().is_some_and(|signal| signal.is_cancelled()) {
            watch.cancel();
            return Err(PlainError::new("The operation was aborted"));
        }
        // `observer.observeCancellation(signal)`: the watch owns its release,
        // so termination also drops the mount observer.
        if let Some(signal) = signal {
            watch.observe_cancellation(signal);
        }
        Ok(watch)
    }

    /// Register an observer created from the current revision, atomically on
    /// the Session line: it sees every later publication and nothing earlier
    /// (`attach`). Returns the revision the observer starts from.
    pub fn attach(
        &self,
        id: ConversationId,
        observer: Arc<dyn ViewObserver>,
        context: &Context,
    ) -> Result<Value, PlainError> {
        let (value, fresh) = self.attach_hydrate(id, context)?;
        self.attach_register(id, &observer, context, fresh)?;
        Ok(value)
    }

    /// The registration tail of `attach` (`attach`'s closed/cancellation
    /// checks, mount install, and observer add).
    fn attach_register(
        &self,
        id: ConversationId,
        observer: &Arc<dyn ViewObserver>,
        context: &Context,
        fresh: Option<(Value, MountedIncarnations)>,
    ) -> Result<(), PlainError> {
        let mut core = self
            .core
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *self
            .closed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            return Err(PlainError::new("Harness is closed"));
        }
        if context
            .abort_signal()
            .is_some_and(|signal| signal.is_cancelled())
        {
            return Err(PlainError::new("The operation was aborted"));
        }
        if let Some((value, docs)) = fresh {
            core.mounts.insert(
                id,
                Mount {
                    value,
                    docs,
                    observers: Vec::new(),
                },
            );
        }
        if let Some(mount) = core.mounts.get_mut(&id) {
            mount.observers.push(Arc::clone(observer));
        }
        Ok(())
    }

    /// Drop an observer and, with the last, its mount (`detach`).
    pub fn detach(&self, id: ConversationId, observer: &Arc<dyn ViewObserver>) {
        let mut core = self
            .core
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let remove = if let Some(mount) = core.mounts.get_mut(&id) {
            mount
                .observers
                .retain(|candidate| !Arc::ptr_eq(candidate, observer));
            mount.observers.is_empty()
        } else {
            false
        };
        if remove {
            core.mounts.remove(&id);
        }
    }

    /// The mount's current revision; `Ok((value, None))` for an existing
    /// mount, `Ok((value, Some((value, docs))))` after hydrating a fresh one
    /// (`attach`'s `get ?? build`).
    fn attach_hydrate(
        &self,
        id: ConversationId,
        context: &Context,
    ) -> Result<HydratedMount, PlainError> {
        let existing = {
            let core = self
                .core
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            core.mounts.get(&id).map(|mount| mount.value.clone())
        };
        if let Some(value) = existing {
            return Ok((value, None));
        }
        let (value, docs) = self.build(id, context)?;
        Ok((value.clone(), Some((value, docs))))
    }

    /// Hydrate a mount's revision and mounted incarnations (`#build`).
    fn build(
        &self,
        id: ConversationId,
        context: &Context,
    ) -> Result<(Value, MountedIncarnations), PlainError> {
        let conversation = self
            .storage
            .conversation(id, context)
            .map_err(|error| PlainError::new(error.to_string()))?;
        let Some(conversation) = conversation else {
            return Err(PlainError::new(format!("Conversation {id} does not exist")));
        };
        let bounds = capture_context_bounds(self.storage.as_ref(), id, context, None)?;
        let entries = active_entries(self.storage.as_ref(), id, bounds, context)?;
        let mut docs: Map<String, Value> = Map::new();
        let mut incarnations: HashMap<String, (i64, i64)> = HashMap::new();
        for definition in [
            conversation_config(),
            live_doc(),
            inbox_doc(),
            super::provider::provider_doc(),
            usage_doc(),
        ] {
            let loaded =
                self.session
                    .conversation_document_on_line(&definition.definition, id, context)?;
            if let Some((record, version, value)) = loaded {
                docs.insert(record.kind.clone(), Value::Object(value));
                incarnations.insert(record.kind, (record.id, version));
            }
        }
        let mut value = Map::new();
        value.insert(
            String::from("conversation"),
            serde_json::to_value(&conversation)
                .map_err(|error| PlainError::new(error.to_string()))?,
        );
        value.insert(
            String::from("entries"),
            Value::Array(
                entries
                    .iter()
                    .map(|entry| serde_json::to_value(entry).unwrap_or(Value::Null))
                    .collect(),
            ),
        );
        value.insert(String::from("docs"), Value::Object(docs));
        Ok((Value::Object(value), incarnations))
    }

    /// Derive the mount's operations from one publication, apply them, and
    /// hand the revision to every observer (`advance`).
    fn advance(&self, id: ConversationId, publication: &CommitPublication, context: &Context) {
        let (before, after, ops) = {
            let mut core = self
                .core
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(mount) = core.mounts.get_mut(&id) else {
                return;
            };
            let mut doc_ops: Vec<Op> = Vec::new();
            let mut entry_ops: Vec<Op> = Vec::new();
            let view = ConversationView::from_json(&mount.value);
            let mut entries = view.entries.clone();
            // Entry writes are published in ID order.
            for change in &publication.changes {
                let CommitChange::Table(super::super::types::TableCommitChange::Entry {
                    value: entry,
                }) = change
                else {
                    continue;
                };
                if entry.conversation_id != id {
                    continue;
                }
                let wire = serde_json::to_value(entry).unwrap_or(Value::Null);
                match entry.head {
                    None => {
                        let index = entries.len();
                        entry_ops.push(Op::Splice {
                            path: vec![Seg::Key(String::from("entries"))],
                            index,
                            remove: 0,
                            items: vec![wire],
                        });
                        entries.push(entry.clone());
                    }
                    // A head marker keeps the non-head entries from its head,
                    // which are always a suffix, and goes in front.
                    Some(target) => {
                        let kept = entries
                            .iter()
                            .position(|candidate| {
                                candidate.head.is_none() && candidate.id >= target
                            })
                            .unwrap_or(entries.len());
                        entry_ops.push(Op::Splice {
                            path: vec![Seg::Key(String::from("entries"))],
                            index: 0,
                            remove: kept,
                            items: vec![serde_json::to_value(entry).unwrap_or(Value::Null)],
                        });
                        let mut next = vec![entry.clone()];
                        next.extend_from_slice(&entries[kept..]);
                        entries = next;
                    }
                }
            }
            for change in &publication.changes {
                let CommitChange::Document(DocumentCommitChange::Document {
                    record,
                    version,
                    value,
                    ops,
                    ..
                }) = change
                else {
                    continue;
                };
                if document_scope_conversation(&record.scope) != Some(id) {
                    continue;
                }
                let kind = record.kind.as_str();
                if !mounted_kinds().contains(kind) || record.key.is_some() {
                    continue;
                }
                let path: Vec<Seg> =
                    vec![Seg::Key(String::from("docs")), Seg::Key(kind.to_owned())];
                let mounted = mount.docs.get(kind).copied();
                if value.is_null() {
                    if mounted.map(|(document_id, _)| document_id) != Some(record.id) {
                        continue;
                    }
                    mount.docs.remove(kind);
                    doc_ops.push(Op::Delete { path });
                } else if mounted.is_some_and(|(document_id, mounted_version)| {
                    document_id == record.id && Some(mounted_version) == *version
                }) {
                    for op in &ops.0 {
                        doc_ops.push(prefixed(op, &path));
                    }
                } else {
                    mount
                        .docs
                        .insert(kind.to_owned(), (record.id, version.unwrap_or_default()));
                    doc_ops.push(Op::Set {
                        path,
                        value: value.clone(),
                    });
                }
            }
            let before = mount.value.clone();
            let ops = {
                let mut combined = doc_ops.clone();
                combined.extend(entry_ops);
                combined
            };
            let frame_value = if doc_ops.is_empty() {
                before.clone()
            } else {
                apply_immutable(Some(&before), &doc_ops).unwrap_or_else(|_| before.clone())
            };
            // Entries were tracked separately, as upstream replaces them
            // wholesale after the doc application.
            let applied = ConversationView::from_json(&frame_value);
            let value = if applied.entries == entries {
                frame_value
            } else {
                let mut next = applied;
                next.entries = entries;
                next.to_json()
            };
            mount.value = value.clone();
            (before, value, ops)
        };
        let frame_context = context.clone().with_value(abort_signal_key(), None);
        if !ops.is_empty() {
            for observer in self.observers_of(id) {
                observer.advance(&after, &ops, &frame_context);
            }
        }
        for observer in self.observers_of(id) {
            observer.publication(&before, &after, &ops, publication, &frame_context);
        }
    }

    fn observers_of(&self, id: ConversationId) -> Vec<Arc<dyn ViewObserver>> {
        self.core
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .mounts
            .get(&id)
            .map(|mount| mount.observers.clone())
            .unwrap_or_default()
    }
}

/// The release closure shared by `state` and `watch`: drop the observer and,
/// with the last observer, its mount (`attach`'s `detach`). The observer is
/// installed into `slot` afterwards with [`install_observer`].
#[allow(clippy::type_complexity)]
fn mount_release(
    views: &Arc<ConversationViews>,
    id: ConversationId,
) -> (
    Arc<Mutex<Option<Weak<dyn ViewObserver>>>>,
    Box<dyn Fn() + Send + Sync>,
) {
    let slot: Arc<Mutex<Option<Weak<dyn ViewObserver>>>> = Arc::new(Mutex::new(None));
    let release_slot = Arc::clone(&slot);
    let views = Arc::downgrade(views);
    let release = Box::new(move || {
        let observer = release_slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let (Some(views), Some(observer)) = (views.upgrade(), observer) {
            if let Some(observer) = observer.upgrade() {
                views.detach(id, &observer);
            }
        }
    });
    (slot, release)
}

/// Install an observer into its release slot (the two-step construction that
/// resolves upstream's closure over the not-yet-created observer).
fn install_observer(
    slot: &Arc<Mutex<Option<Weak<dyn ViewObserver>>>>,
    observer: &Arc<dyn ViewObserver>,
) {
    *slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::downgrade(observer));
}

/// `ConversationViews.state()` mounts the view revision on a
/// [`CommittedStateSource`]: the source is the mount observer, forwarding
/// revisions and the Session close (`attach`'s
/// `(value, release) => new CommittedStateSource(value, release)`).
impl ViewObserver for CommittedStateSource {
    fn advance(&self, value: &Value, ops: &[Op], context: &Context) {
        CommittedStateSource::advance(self, value.clone(), ops.to_vec(), context.clone());
    }

    fn publication(
        &self,
        _before: &Value,
        _after: &Value,
        _ops: &[Op],
        _publication: &CommitPublication,
        _context: &Context,
    ) {
    }

    fn close_session(&self) {
        CommittedStateSource::close_session(self);
    }
}

/// `op` moved under `prefix`; a root replacement becomes a set of the prefix
/// (`prefixed`).
fn prefixed(op: &Op, prefix: &[Seg]) -> Op {
    let at = |path: &[Seg]| -> Vec<Seg> {
        let mut moved = prefix.to_vec();
        moved.extend(path.iter().cloned());
        moved
    };
    match op {
        Op::Replace(value) => Op::Set {
            path: prefix.to_vec(),
            value: value.clone(),
        },
        Op::Set { path, value } => Op::Set {
            path: at(path),
            value: value.clone(),
        },
        Op::Delete { path } => Op::Delete { path: at(path) },
        Op::Append { path, text } => Op::Append {
            path: at(path),
            text: text.clone(),
        },
        Op::Truncate { path, count } => Op::Truncate {
            path: at(path),
            count: *count,
        },
        Op::Splice {
            path,
            index,
            remove,
            items,
        } => Op::Splice {
            path: at(path),
            index: *index,
            remove: *remove,
            items: items.clone(),
        },
        Op::Reorder { path, permutation } => Op::Reorder {
            path: at(path),
            permutation: permutation.clone(),
        },
    }
}

/// Watch-end re-export for the docs.
pub type ViewWatchEnd = WatchEnd;

/// The conversation a document scope belongs to (`change.conversationId`).
fn document_scope_conversation(
    scope: &super::super::types::DocumentScope,
) -> Option<ConversationId> {
    match scope {
        super::super::types::DocumentScope::Conversation { conversation_id } => {
            Some(*conversation_id)
        }
        super::super::types::DocumentScope::Task { .. }
        | super::super::types::DocumentScope::Session => None,
    }
}

/// The watch adapter: a mount observer that forwards revisions and the
/// Session close into a [`CommittedWatch`] (`attach`'s
/// `(value, release) => new CommittedWatch(value, release)`).
struct WatchObserver {
    watch: Arc<CommittedWatch>,
}

impl ViewObserver for WatchObserver {
    fn advance(&self, value: &Value, ops: &[Op], context: &Context) {
        self.watch
            .advance(value.clone(), ops.to_vec(), context.clone());
    }

    fn publication(
        &self,
        _before: &Value,
        _after: &Value,
        _ops: &[Op],
        _publication: &CommitPublication,
        _context: &Context,
    ) {
    }

    fn close_session(&self) {
        self.watch.close_session();
    }
}
