//! Port of `packages/agent/src/harness/pico3/session.ts` (1497 lines): the
//! line-serialized Session — document cache and defaults ([`Defaults`],
//! [`Docs`]), the capability-checked transaction ([`Tx`]), the conversation
//! index, and the commit pipeline.
//!
//! # Disclosed substitutions
//!
//! - **Line serialization.** Upstream chains operations on a promise tail
//!   (`enter`, `session.ts:1474-1489`) and detects reentry through a context
//!   value. The port keeps the context-value check ([`line_key`], rejecting
//!   [`nested_line_operation`]) and serializes with a tokio mutex.
//! - **Trackers move instead of aliasing.** Upstream's `Docs` cache and each
//!   transaction share tracker objects (JS references). The port moves a
//!   tracker out of the cache while a transaction touches it and returns the
//!   touched trackers to the cache when the commit succeeds; a failed
//!   transaction evicts them (upstream `evictTouched`, `session.ts:1174-1176`).
//!   Under the line lock at most one transaction exists at a time, so the
//!   observable cache behavior is identical.
//! - **Callback surface.** Upstream hands callbacks a frozen
//!   null-prototype `Proxy` limited to the allowed methods
//!   (`callbackSurface`, `session.ts:339-358`) that throws after
//!   `closeSurface()`. The port's capability boundary is API visibility:
//!   host operations are `pub`, core operations are `pub(crate)`, the
//!   surface-active check stays, and "a handle cannot outlive its callback"
//!   is the borrow checker. `TransactionControl` (the separate control
//!   object terminal closures use) collapses into
//!   [`Tx::set_task_for_control`] for the same reason.
//! - **Owner registry.** Upstream keeps `owners` in a `WeakMap` keyed by the
//!   Storage object (`session.ts:1196`); the port keys a process-global map
//!   by the storage's pointer address, removed on close, fault, and `Drop`.
//! - **Namespace views.** Upstream `plugins()` returns a proxy with live
//!   accessors and caches one view per namespace per transaction
//!   (`session.ts:289-294`); the port returns an explicit
//!   [`PluginsView`] handle — every access goes through the same slices, so
//!   behavior matches without the cache.
//! - `structuredClone`/`JSON.parse(JSON.stringify)` deep copies become serde
//!   round-trips ([`plain`]); the patch diff (`session.ts:771-781`) compares
//!   serde values.
//! - Upstream `removeWhere` (`session.ts:62-64`) is [`Vec::retain`] applied
//!   with a negated predicate; same in-place result.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use futures::future::BoxFuture;
use futures::FutureExt;
use serde_json::{json, Map, Value};

use crate::agent_core::chord_support::delta::{is_base, op_to_json, track, Op, Tracker};
use crate::agent_core::chord_support::{create_context_key, Context, ContextKey};
use crate::agent_core::harness::context::without_abort_signal;
use crate::ai::now_ms;

use super::context::derive_context;
use super::membrane::Membrane;
use super::types::{
    closed, collapse_in_progress, conversation_busy, faulted, forbidden, generation_in_progress,
    is_core_kind, nested_line_operation, number, ContextView, Conversation, ConversationParentSpec,
    ConversationSpec, DocRef, Entry, EntryScan, Head, Id, Input, InvocationMode, Invoker,
    JsonObject, Namespace, NamespaceRegistration, NewEntry, OwnedConversationSpec, ParentAt,
    ReadAfterWrite, SendInput, Seq, Storage, Task, TaskPatch, TaskScan, TaskSpec, TaskStatus,
    ViewEvent, Write,
};

// ---------------------------------------------------------------------------
// Document cache and defaults
// ---------------------------------------------------------------------------

/// Upstream `CORE_KINDS` (`session.ts:66`).
pub use super::types::is_core_kind as is_core_kind_name;

/// Upstream `docKey` (`session.ts:59`).
fn doc_key(reference: &DocRef) -> String {
    reference.key()
}

/// Upstream `Defaults` (`session.ts:105-175`): declared config defaults,
/// derived from the registered kinds. Nothing is duplicated elsewhere.
#[derive(Debug, Default, Clone)]
pub struct Defaults {
    pub rewindable: JsonObject,
    pub sticky: JsonObject,
    /// Config key -> declaring document (`"rewindable" | "sticky"`).
    pub route: HashMap<String, String>,
    /// Config key -> owning kind name (upstream: the kind object).
    owners: HashMap<String, String>,
}

fn finite_nonnegative(value: &Value) -> bool {
    value
        .as_f64()
        .is_some_and(|number| number.is_finite() && number >= 0.0)
}

/// Upstream `exactObject` (`session.ts:90-101`).
fn exact_object(value: &Value, required: &[&str], optional: &[&str]) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    if !required.iter().all(|key| object.contains_key(*key)) {
        return false;
    }
    object
        .keys()
        .all(|key| required.contains(&key.as_str()) || optional.contains(&key.as_str()))
}

/// Upstream `CORE_CONFIG_VALIDATORS` (`session.ts:67-85`). Returns `None`
/// when the key has no core validator.
fn core_config_validator(key: &str, value: &Value) -> Option<bool> {
    match key {
        "model" => Some(
            exact_object(value, &["provider", "modelId"], &[])
                && value.get("provider").and_then(Value::as_str) != Some("")
                && value.get("modelId").and_then(Value::as_str) != Some(""),
        ),
        "thinkingLevel" => Some(matches!(
            value.as_str(),
            Some("off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max")
        )),
        "selectedTools" => Some(
            value
                .as_array()
                .is_some_and(|names| names.iter().all(Value::is_string)),
        ),
        "profile" => Some(value.is_string()),
        "retry" => Some(
            exact_object(
                value,
                &["enabled", "maxRetries", "baseDelayMs"],
                &["maxAgentDelayMs"],
            ) && value.get("enabled").is_some_and(Value::is_boolean)
                && value
                    .get("maxRetries")
                    .and_then(Value::as_f64)
                    .is_some_and(|max| max.fract() == 0.0 && max >= 0.0)
                && value.get("baseDelayMs").is_some_and(finite_nonnegative)
                && value
                    .get("maxAgentDelayMs")
                    .map(finite_nonnegative)
                    .unwrap_or(true),
        ),
        "threshold" => Some(value.as_f64().is_some_and(|number| number.is_finite())),
        "keepRecent" => Some(finite_nonnegative(value)),
        "steeringMode" | "followUpMode" => {
            Some(matches!(value.as_str(), Some("all" | "one-at-a-time")))
        }
        _ => None,
    }
}

impl Defaults {
    /// Upstream `new Defaults(kinds)` (`session.ts:110-112`).
    pub fn new(kinds: impl Iterator<Item = Arc<dyn super::types::AnyKind>>) -> Defaults {
        let mut defaults = Defaults::default();
        for kind in kinds {
            defaults.register(&kind);
        }
        defaults
    }

    /// Upstream `register` (`session.ts:113-128`): a key declared by more
    /// than one kind is an error.
    pub fn register(&mut self, kind: &Arc<dyn super::types::AnyKind>) {
        let mut declarations: Vec<(String, String, Option<Value>)> = Vec::new();
        for (doc, declared) in [
            ("rewindable", kind.config().map(|config| &config.rewindable)),
            ("sticky", kind.config().map(|config| &config.sticky)),
        ] {
            if let Some(declared) = declared {
                for (key, value) in declared {
                    declarations.push((doc.to_owned(), key.clone(), Some(value.clone())));
                }
            }
        }
        // Task 9: upstream declares routed-but-valueless keys as
        // `key: undefined` (`kinds/generation.ts:80`); the port lists them in
        // `KindConfig::declared_absent` — same route, no seeded value.
        for (doc, absent) in [
            (
                "rewindable",
                kind.config()
                    .map(|config| &config.declared_absent.rewindable),
            ),
            (
                "sticky",
                kind.config().map(|config| &config.declared_absent.sticky),
            ),
        ] {
            if let Some(absent) = absent {
                for key in absent {
                    declarations.push((doc.to_owned(), key.clone(), None));
                }
            }
        }
        let mut local: HashSet<String> = HashSet::new();
        for (doc, key, _) in &declarations {
            let _ = doc;
            if local.contains(key) || self.route.contains_key(key) {
                panic!("config key \"{key}\" declared by more than one kind");
            }
            local.insert(key.clone());
        }
        for (doc, key, value) in declarations {
            self.route.insert(key.clone(), doc.clone());
            self.owners.insert(key.clone(), kind.name().to_owned());
            let Some(value) = value else {
                // Declared absent: routed, but seeds nothing
                // (`session.ts:123-127` skips `undefined`).
                continue;
            };
            // Upstream skips `undefined`, not null: a declared null default
            // is a value (`session.ts:123-127`).
            let target = if doc == "rewindable" {
                &mut self.rewindable
            } else {
                &mut self.sticky
            };
            target.insert(key, plain(&value));
        }
    }

    /// Upstream `unregister` (`session.ts:129-137`): removes only the keys
    /// this kind owns. Kind identity is the name (unique among registered
    /// kinds).
    pub fn unregister(&mut self, kind: &Arc<dyn super::types::AnyKind>) {
        let owned: Vec<String> = self
            .owners
            .iter()
            .filter(|(_, owner)| owner.as_str() == kind.name())
            .map(|(key, _)| key.clone())
            .collect();
        for key in owned {
            if let Some(doc) = self.route.remove(&key) {
                let target = if doc == "rewindable" {
                    &mut self.rewindable
                } else {
                    &mut self.sticky
                };
                target.shift_remove(&key);
            }
            self.owners.remove(&key);
        }
    }

    /// Upstream `validate` (`session.ts:138-143`).
    pub fn validate(&self, key: &str, value: &Value) -> bool {
        if let Some(validator) = core_config_validator(key, value) {
            return validator;
        }
        self.route.contains_key(key)
    }

    /// Upstream `validateSeed` (`session.ts:144-153`).
    pub fn validate_seed(&self, doc: &str, seed: &JsonObject) -> anyhow::Result<JsonObject> {
        let mut out = JsonObject::new();
        for (key, value) in seed {
            if self.route.get(key).map(String::as_str) != Some(doc) || !self.validate(key, value) {
                anyhow::bail!("invalid {doc} config value for \"{key}\"");
            }
            out.insert(key.clone(), plain(value));
        }
        Ok(out)
    }

    /// Upstream `fill` (`session.ts:154-157`): fill declared keys that are
    /// absent (never `??`: a stored null is a value).
    pub fn fill(&self, doc: &str, target: &mut JsonObject) {
        let source = if doc == "rewindable" {
            &self.rewindable
        } else {
            &self.sticky
        };
        for (key, value) in source {
            if !target.contains_key(key) {
                target.insert(key.clone(), plain(value));
            }
        }
    }

    /// Upstream `freshRewindable` (`session.ts:158-168`).
    pub fn fresh_rewindable(
        &self,
        over: &JsonObject,
        preserve_plugins: bool,
    ) -> anyhow::Result<JsonObject> {
        let mut base = JsonObject::from_iter([("plugins".to_owned(), Value::Object(Map::new()))]);
        self.fill("rewindable", &mut base);
        let mut over = over.clone();
        let plugins = over.shift_remove("plugins");
        let rest = if preserve_plugins {
            over
        } else {
            self.validate_seed("rewindable", &over)?
        };
        base.extend(rest);
        if preserve_plugins {
            if let Some(plugins) = plugins {
                base.insert("plugins".to_owned(), plain(&plugins));
            }
        }
        Ok(base)
    }

    /// Upstream `freshSticky` (`session.ts:169-174`).
    pub fn fresh_sticky(&self, over: &JsonObject) -> anyhow::Result<JsonObject> {
        let mut base: JsonObject = JsonObject::from_iter([
            ("inbox".to_owned(), Value::Array(Vec::new())),
            (
                "turn".to_owned(),
                Value::Object(JsonObject::from_iter([(
                    "tools".to_owned(),
                    Value::Array(Vec::new()),
                )])),
            ),
            ("tasks".to_owned(), Value::Object(Map::new())),
            ("plugins".to_owned(), Value::Object(Map::new())),
        ]);
        self.fill("sticky", &mut base);
        let mut over = over.clone();
        for reserved in ["inbox", "turn", "tasks", "plugins"] {
            over.shift_remove(reserved);
        }
        let rest = self.validate_seed("sticky", &over)?;
        base.extend(rest);
        Ok(base)
    }
}

/// Upstream `Docs` (`session.ts:177-218`): the session-level document cache.
#[derive(Default)]
pub struct Docs {
    cache: HashMap<String, Tracker>,
    /// Bytes of ops written since the last base, per document
    /// (`session.ts:181-182`). Drives rebase+truncate.
    pub since_base: HashMap<String, usize>,
}

impl Docs {
    /// Upstream `noteOps` (`session.ts:187-190`).
    pub fn note_ops(&mut self, key: &str, ops: &[Op]) {
        let bytes = serde_json::to_vec(&ops.iter().map(op_to_json).collect::<Vec<_>>())
            .map(|bytes| bytes.len())
            .unwrap_or(0);
        let next = if is_base(ops) {
            0
        } else {
            self.since_base.get(key).copied().unwrap_or(0) + bytes
        };
        self.since_base.insert(key.to_owned(), next);
    }

    /// Upstream `requestBase` (`session.ts:191-193`).
    pub fn request_base(&mut self, key: &str) {
        if let Some(tracker) = self.cache.get_mut(key) {
            tracker.rebase();
        }
    }

    /// Upstream `evict` (`session.ts:206-208`).
    pub fn evict(&mut self, key: &str) {
        self.cache.remove(key);
    }

    /// Upstream `loaded` (`session.ts:212-214`).
    pub fn loaded(&self, key: &str) -> Option<JsonObject> {
        self.cache.get(key).map(|tracker| match tracker.state() {
            Value::Object(object) => object.clone(),
            _ => JsonObject::new(),
        })
    }
}

/// Session-side conversation index (`session.ts:233-240`): owner/parent
/// graph for subtree checks and ancestry.
pub trait ConversationIndex {
    fn get(&self, id: Id) -> Option<Conversation>;
    /// Conversation ids in the subtree rooted at `root` (inclusive), via
    /// ownership.
    fn subtree(&self, root: Id) -> HashSet<Id>;
    /// Conversation ids from the root down to `id` (exclusive of `id`) via
    /// ownership.
    fn ancestors(&self, id: Id) -> Vec<Id>;
}

/// Upstream `CommitChanges` (`session.ts:224-231`).
#[derive(Debug, Default, Clone)]
pub struct CommitChanges {
    pub entries: Vec<Entry>,
    pub tasks: Vec<Task>,
    pub inputs: Vec<Input>,
    pub conversations: Vec<Conversation>,
    pub docs: Vec<(DocRef, Vec<Op>)>,
    pub events: Vec<(Id, ViewEvent)>,
}

/// The listener notification payload: what a finished commit did. Upstream
/// listeners receive the whole `CommitResult<unknown>`
/// (`session.ts:1228-1229`); the value is type-erased there, so the port
/// hands out seq + changes, which is all listeners read.
#[derive(Debug, Clone)]
pub struct CommitRecord {
    pub seq: Option<Seq>,
    pub changes: CommitChanges,
}

/// Upstream `CommitResult<T>` (`session.ts:1205-1209`).
#[derive(Debug)]
pub struct CommitResult<T> {
    pub value: T,
    pub seq: Option<Seq>,
    pub changes: CommitChanges,
}

/// A task reference carrying the registered kind token
/// (`types.ts:516-519`).
#[derive(Clone)]
pub struct TaskRef {
    pub id: Id,
    pub kind: Arc<dyn super::types::AnyKind>,
}

/// The doc refs a commit must preload (`session.ts:1314-1319`).
#[derive(Debug, Default, Clone)]
pub struct CommitOptions {
    pub docs: Vec<DocRef>,
    pub closing: bool,
}

/// The line key (`session.ts:1195`): marks a context as running on the line.
fn line_key() -> &'static ContextKey<bool> {
    static KEY: OnceLock<ContextKey<bool>> = OnceLock::new();
    KEY.get_or_init(|| create_context_key("pico3.session.line"))
}

/// Upstream `NestedLineOperation` (`session.ts:1198-1203`).
pub fn nested_line_operation_error() -> anyhow::Error {
    nested_line_operation()
}

/// The session's error-reporter handle (upstream `onReport`,
/// `session.ts:1230-1231`).
type OnReport = Arc<dyn Fn(&anyhow::Error) + Send + Sync>;
/// A commit listener (upstream `lineListeners`/`listeners`,
/// `session.ts:1228-1229`).
type CommitListener = Arc<dyn Fn(&CommitRecord) + Send + Sync>;

/// The mutable session state, guarded for the line.
#[derive(Default)]
struct SessionState {
    live_tasks: HashMap<Id, Task>,
    conversation_records: HashMap<Id, Conversation>,
    /// Owner tasks that are terminal but whose conversations still exist
    /// (ancestry after reopen) (`session.ts:1279-1280`).
    owner_task_cache: HashMap<Id, Task>,
    docs: Docs,
    closed: bool,
    /// The stored fault once persistence failed (`session.ts:1221`).
    fault: Option<String>,
}

/// The kind registry. Task-9 note (disclosed): upstream `this.kinds` is a
/// mutable `Map` the harness mutates through `registerTaskKind`
/// (`harness.ts:427-439`); the port holds it behind a lock so the Session
/// view/defaults projections see runtime registrations.
type KindRegistry = Arc<RwLock<HashMap<String, Arc<dyn super::types::AnyKind>>>>;

/// Upstream `Session` (`session.ts:1215-1495`): the line.
pub struct Session {
    storage: Arc<dyn Storage>,
    kinds: KindRegistry,
    namespaces: Arc<RwLock<HashMap<String, NamespaceRegistration>>>,
    state: Mutex<SessionState>,
    /// The serialized line (upstream `tail`, `session.ts:1219`).
    line: tokio::sync::Mutex<()>,
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
    /// Upstream `onReport` (`session.ts:1230-1231`): errors from listeners
    /// and post-commit work; never surfaces to the writer.
    on_report: RwLock<OnReport>,
    /// Upstream `lineListeners` (`session.ts:1228`).
    line_listeners: RwLock<Vec<CommitListener>>,
    /// Upstream `listeners` (`session.ts:1229`).
    listeners: RwLock<Vec<CommitListener>>,
}

/// The process-global owner registry (upstream `owners` WeakMap,
/// `session.ts:1196`), keyed by storage address.
fn owned_storages() -> &'static Mutex<HashSet<usize>> {
    static OWNED: OnceLock<Mutex<HashSet<usize>>> = OnceLock::new();
    OWNED.get_or_init(|| Mutex::new(HashSet::new()))
}

fn storage_key(storage: &Arc<dyn Storage>) -> usize {
    Arc::as_ptr(storage) as *const () as usize
}

/// A plain JSON copy (`session.ts:1180`).
pub(crate) fn plain(value: &Value) -> Value {
    serde_json::from_slice(&serde_json::to_vec(value).expect("values serialize"))
        .expect("values round-trip")
}

/// Deep-copy helper for record structs (upstream `plain(task)` /
/// `structuredClone`).
pub(crate) fn plain_record<T: serde::Serialize + serde::de::DeserializeOwned>(value: &T) -> T {
    serde_json::from_slice(&serde_json::to_vec(value).expect("records serialize"))
        .expect("records round-trip")
}

/// Upstream `validateEntry` (`session.ts:1182-1189`). The final
/// `JSON.stringify` strictness check needs no port: records are serde
/// values, so cycles and BigInt cannot exist.
fn validate_entry(entry: &Entry) -> anyhow::Result<()> {
    if let Some(head) = entry.head {
        if head > entry.id {
            anyhow::bail!("entry {}: head {head} is in the future", entry.id);
        }
    }
    if let Some(model) = &entry.model {
        for message in model {
            if !message.is_object() || message.get("role").and_then(Value::as_str).is_none() {
                anyhow::bail!("entry {}: malformed model message", entry.id);
            }
        }
    }
    Ok(())
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "listener panicked".to_owned()
    }
}

impl Session {
    /// Upstream `new Session(...)` (`session.ts:1233-1247`): one owning
    /// Session per Storage object.
    pub fn new(
        storage: Arc<dyn Storage>,
        kinds: HashMap<String, Arc<dyn super::types::AnyKind>>,
        namespaces: HashMap<String, NamespaceRegistration>,
    ) -> anyhow::Result<Arc<Session>> {
        Session::with_clock(storage, kinds, namespaces, Arc::new(now_ms))
    }

    /// The constructor with an injectable clock (upstream `now` parameter,
    /// default `Date.now`).
    pub fn with_clock(
        storage: Arc<dyn Storage>,
        kinds: HashMap<String, Arc<dyn super::types::AnyKind>>,
        namespaces: HashMap<String, NamespaceRegistration>,
        now: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> anyhow::Result<Arc<Session>> {
        let mut owned = owned_storages().lock().expect("owner registry");
        let key = storage_key(&storage);
        if owned.contains(&key) {
            anyhow::bail!("this Storage already has an owning Session");
        }
        owned.insert(key);
        drop(owned);
        Ok(Arc::new(Session {
            storage,
            kinds: Arc::new(RwLock::new(kinds)),
            namespaces: Arc::new(RwLock::new(namespaces)),
            state: Mutex::new(SessionState::default()),
            line: tokio::sync::Mutex::new(()),
            now,
            on_report: RwLock::new(Arc::new(|_| {})),
            line_listeners: RwLock::new(Vec::new()),
            listeners: RwLock::new(Vec::new()),
        }))
    }

    /// Upstream `readonly storage`.
    pub fn storage(&self) -> &Arc<dyn Storage> {
        &self.storage
    }

    /// Upstream `readonly kinds`.
    pub fn kinds(&self) -> KindRegistry {
        self.kinds.clone()
    }

    /// Register a kind at runtime (`harness.ts:431-432` /
    /// `session.defaults.register(kind)`): the Session keeps the registry
    /// the view and defaults derive from.
    pub fn register_kind(&self, kind: Arc<dyn super::types::AnyKind>) -> anyhow::Result<()> {
        let mut kinds = self.kinds.write().expect("kind registry");
        if kinds.contains_key(kind.name()) {
            anyhow::bail!("task kind \"{}\" already registered", kind.name());
        }
        kinds.insert(kind.name().to_owned(), kind);
        Ok(())
    }

    /// Remove a runtime registration (`harness.ts:434-438`).
    pub fn unregister_kind(&self, name: &str) {
        self.kinds.write().expect("kind registry").remove(name);
    }

    /// Upstream `readonly namespaces`.
    pub fn namespaces(&self) -> &Arc<RwLock<HashMap<String, NamespaceRegistration>>> {
        &self.namespaces
    }

    /// Upstream `readonly defaults` (`session.ts:1224`), derived from the
    /// registered kinds at construction (`session.ts:1244`).
    pub fn defaults(&self) -> Defaults {
        Defaults::new(self.kinds.read().expect("kind registry").values().cloned())
    }

    /// Upstream `now` (`session.ts:1222`).
    pub fn now(&self) -> i64 {
        (self.now)()
    }

    /// Upstream `onReport` assignment (`session.ts:1230-1231`).
    pub fn set_on_report(&self, on_report: Arc<dyn Fn(&anyhow::Error) + Send + Sync>) {
        *self.on_report.write().expect("on_report lock") = on_report;
    }

    fn report(&self, error: &anyhow::Error) {
        let handler = self.on_report.read().expect("on_report lock").clone();
        (handler)(error);
    }

    /// Crate-visible [`Session::report`] for the view manager's listener
    /// failure reporting (`view.ts:330-334`).
    pub(crate) fn report_public(&self, error: &anyhow::Error) {
        self.report(error);
    }

    /// Register a line listener (`session.ts:1228`): fires on the line after
    /// persistence, when a seq exists.
    pub fn add_line_listener(&self, listener: Arc<dyn Fn(&CommitRecord) + Send + Sync>) {
        self.line_listeners
            .write()
            .expect("listeners")
            .push(listener);
    }

    /// Register a post-line listener (`session.ts:1229`).
    pub fn add_listener(&self, listener: Arc<dyn Fn(&CommitRecord) + Send + Sync>) {
        self.listeners.write().expect("listeners").push(listener);
    }

    /// Upstream `liveTasks` snapshot.
    pub fn live_tasks(&self) -> HashMap<Id, Task> {
        self.state.lock().expect("session state").live_tasks.clone()
    }

    /// Harness-open population (`harness.ts:345-355`): records and live
    /// tasks recovered from storage are inserted into the session's
    /// in-memory indexes. Crate-visible: the harness constructs them before
    /// any commit runs.
    pub(crate) fn seed_recovered_state(
        &self,
        conversations: Vec<Conversation>,
        live_tasks: Vec<Task>,
        owner_tasks: Vec<Task>,
    ) {
        let mut state = self.state.lock().expect("session state");
        for conversation in conversations {
            state
                .conversation_records
                .insert(conversation.id, conversation);
        }
        for task in live_tasks {
            state.live_tasks.insert(task.id, task);
        }
        for task in owner_tasks {
            state.owner_task_cache.insert(task.id, task);
        }
    }

    /// Upstream `conversationRecords` snapshot.
    pub fn conversation_records(&self) -> HashMap<Id, Conversation> {
        self.state
            .lock()
            .expect("session state")
            .conversation_records
            .clone()
    }

    /// Upstream `index` (`session.ts:1249-1278`).
    pub fn index(&self) -> impl ConversationIndex + '_ {
        struct Index<'a>(&'a Session);
        impl ConversationIndex for Index<'_> {
            fn get(&self, id: Id) -> Option<Conversation> {
                self.0
                    .state
                    .lock()
                    .expect("session state")
                    .conversation_records
                    .get(&id)
                    .cloned()
            }
            fn subtree(&self, root: Id) -> HashSet<Id> {
                let state = self.0.state.lock().expect("session state");
                let mut out: HashSet<Id> = HashSet::from([root]);
                let mut grew = true;
                while grew {
                    grew = false;
                    for conversation in state.conversation_records.values() {
                        let Some(owner) = conversation.owner else {
                            continue;
                        };
                        if out.contains(&conversation.id) {
                            continue;
                        }
                        let owner_task = state
                            .live_tasks
                            .get(&owner)
                            .or_else(|| state.owner_task_cache.get(&owner));
                        if owner_task.is_some_and(|task| out.contains(&task.conversation_id)) {
                            out.insert(conversation.id);
                            grew = true;
                        }
                    }
                }
                out
            }
            fn ancestors(&self, id: Id) -> Vec<Id> {
                let state = self.0.state.lock().expect("session state");
                let mut chain: Vec<Id> = Vec::new();
                let mut current = state.conversation_records.get(&id);
                while let Some(conversation) = current {
                    let Some(owner) = conversation.owner else {
                        break;
                    };
                    let Some(task) = state
                        .live_tasks
                        .get(&owner)
                        .or_else(|| state.owner_task_cache.get(&owner))
                    else {
                        break;
                    };
                    chain.insert(0, task.conversation_id);
                    current = state.conversation_records.get(&task.conversation_id);
                }
                chain
            }
        }
        Index(self)
    }

    /// Upstream `commit` (`session.ts:1282-1382`). The callback receives the
    /// transaction and the line context; the commit's value comes back
    /// alongside the seq and the change record.
    pub async fn commit<T>(
        self: &Arc<Self>,
        invoker: Invoker,
        ctx: Context,
        opts: CommitOptions,
        f: impl for<'tx> FnOnce(&'tx mut Tx, Context) -> BoxFuture<'tx, anyhow::Result<T>>,
    ) -> anyhow::Result<CommitResult<T>> {
        // ctx.abortSignal?.throwIfAborted() (`session.ts:1288`).
        if let Some(signal) = ctx.abort_signal() {
            if signal.is_cancelled() {
                anyhow::bail!("aborted");
            }
        }
        let line_ctx = self.derive_line_ctx(&ctx)?;
        let result = self.commit_on_line(invoker, line_ctx, opts, f).await;
        match result {
            Ok(result) => {
                if result.seq.is_some() {
                    let record = CommitRecord {
                        seq: result.seq,
                        changes: result.changes.clone(),
                    };
                    for listener in self.listeners.read().expect("listeners").clone() {
                        // Listener exceptions never reach the writer
                        // (`session.ts:1373-1380`).
                        if let Err(payload) =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                listener(&record)
                            }))
                        {
                            let error = anyhow::Error::msg(panic_message(payload));
                            self.report(&error);
                        }
                    }
                }
                Ok(result)
            }
            Err(error) => Err(error),
        }
    }

    /// The reentry check plus the line-stamped context (`enter`,
    /// `session.ts:1474-1489`, minus the tail chaining — the port's line ops
    /// each take [`Session::line`]).
    fn derive_line_ctx(&self, ctx: &Context) -> anyhow::Result<Context> {
        if ctx.get(line_key()).is_some() {
            return Err(nested_line_operation_error());
        }
        Ok(ctx.clone().with_value(line_key(), true))
    }

    async fn commit_on_line<T>(
        self: &Arc<Self>,
        invoker: Invoker,
        line_ctx: Context,
        opts: CommitOptions,
        f: impl for<'tx> FnOnce(&'tx mut Tx, Context) -> BoxFuture<'tx, anyhow::Result<T>>,
    ) -> anyhow::Result<CommitResult<T>> {
        let _line = self.line.lock().await;
        self.assert_usable()?;
        // lineCtx.abortSignal?.throwIfAborted() (`session.ts:1292`).
        if let Some(signal) = line_ctx.abort_signal() {
            if signal.is_cancelled() {
                anyhow::bail!("aborted");
            }
        }
        if let Invoker::Task {
            token, id, mode, ..
        } = &invoker
        {
            // A captured runtime cannot write after its invocation returned,
            // after terminalization, or after a mark (run mode)
            // (`session.ts:1293-1300`).
            if !token.alive() {
                return Err(forbidden("commit from a finished invocation"));
            }
            let live = self
                .state
                .lock()
                .expect("session state")
                .live_tasks
                .get(id)
                .cloned();
            let Some(live) = live else {
                return Err(forbidden("commit from a task that is not live"));
            };
            if *mode == InvocationMode::Run && live.abort == Some(true) {
                return Err(forbidden("commit from a marked run invocation"));
            }
        }
        let mut tx = Tx::new(&invoker, self.clone(), line_ctx.clone());
        tx.closing = opts.closing;
        let mut refs = vec![DocRef::Session];
        refs.extend(opts.docs.iter().copied());
        if let Invoker::Task {
            conversation_id, ..
        } = &invoker
        {
            refs.push(DocRef::Rewindable {
                conversation_id: *conversation_id,
            });
            refs.push(DocRef::Sticky {
                conversation_id: *conversation_id,
            });
        }
        let mut persisted = false;
        let outcome: anyhow::Result<(T, Option<Seq>)> = async {
            tx.preload(refs, line_ctx.clone()).await?;
            let value = {
                let future = f(&mut tx, line_ctx.clone());
                let outcome = future.await;
                // finally { tx.closeSurface() } (`session.ts:1328-1330`).
                tx.close_surface();
                outcome?
            };
            // Preload the sticky docs of every conversation whose tasks
            // changed (`session.ts:1331-1336`).
            let mut changed_conversations: Vec<Id> = tx
                .changes
                .tasks
                .iter()
                .map(|task| task.conversation_id)
                .collect();
            changed_conversations.sort_unstable();
            changed_conversations.dedup();
            tx.preload(
                changed_conversations
                    .into_iter()
                    .map(|conversation_id| DocRef::Sticky { conversation_id })
                    .collect(),
                line_ctx.clone(),
            )
            .await?;
            let writes = tx.finish()?;
            let mut seq: Option<Seq> = None;
            if !writes.is_empty() || !tx.changes.events.is_empty() {
                persisted = true;
                match self
                    .storage
                    .commit(writes, without_abort_signal(line_ctx.clone()))
                    .await
                {
                    Ok(committed) => seq = Some(committed),
                    Err(error) => {
                        // The fault path (`session.ts:1343-1350`): store the
                        // fault, close the storage, release ownership.
                        let fault = faulted(error);
                        let _ = self
                            .storage
                            .close(without_abort_signal(line_ctx.clone()))
                            .await;
                        owned_storages()
                            .lock()
                            .expect("owner registry")
                            .remove(&storage_key(&self.storage));
                        self.state
                            .lock()
                            .expect("session state")
                            .fault
                            .replace(format!("{fault}"));
                        return Err(fault);
                    }
                }
                self.apply_changes(&tx.changes);
            }
            Ok((value, seq))
        }
        .await;
        // finally { tx.revoke() } (`session.ts:1369-1371`).
        tx.revoke();
        match outcome {
            Ok((value, seq)) => {
                tx.return_touched_to_cache();
                if seq.is_some() {
                    let record = CommitRecord {
                        seq,
                        changes: tx.changes.clone(),
                    };
                    for listener in self.line_listeners.read().expect("listeners").clone() {
                        if let Err(payload) =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                listener(&record)
                            }))
                        {
                            let error = anyhow::Error::msg(panic_message(payload));
                            self.report(&error);
                        }
                    }
                }
                Ok(CommitResult {
                    value,
                    seq,
                    changes: tx.changes,
                })
            }
            Err(error) => {
                // catch { if (!persisted) tx.evictTouched(); throw }
                // (`session.ts:1366-1368`).
                if !persisted {
                    tx.evict_touched();
                } else {
                    tx.return_touched_to_cache();
                }
                Err(error)
            }
        }
    }

    /// Read-only line operation without a transaction (`session.ts:1384-1390`).
    pub async fn read<T>(
        &self,
        f: impl FnOnce(Arc<dyn Storage>, Context) -> BoxFuture<'static, anyhow::Result<T>>,
        ctx: Context,
    ) -> anyhow::Result<T> {
        let line_ctx = self.derive_line_ctx(&ctx)?;
        let _line = self.line.lock().await;
        self.assert_usable()?;
        f(self.storage.clone(), line_ctx).await
    }

    /// Generic line operation (`session.ts:1391-1397`): waiter registration
    /// and asynchronous lifecycle work.
    pub async fn on_line<T>(
        &self,
        f: impl FnOnce(Context) -> BoxFuture<'static, anyhow::Result<T>>,
        ctx: Context,
    ) -> anyhow::Result<T> {
        let line_ctx = self.derive_line_ctx(&ctx)?;
        let _line = self.line.lock().await;
        self.assert_usable()?;
        f(line_ctx).await
    }

    /// Upstream `STICKY_BASE_BUDGET` (`session.ts:1399`).
    pub const STICKY_BASE_BUDGET: usize = 256 * 1024;

    /// Upstream `retire` (`session.ts:1401-1416`): after a task
    /// terminalizes, retire its slot; base + truncate when idle or over
    /// budget. Truncation runs on the line.
    pub async fn retire(self: &Arc<Self>, task: &Task, ctx: Context) -> anyhow::Result<()> {
        let reference = DocRef::Sticky {
            conversation_id: task.conversation_id,
        };
        let (idle, over) = {
            let state = self.state.lock().expect("session state");
            let idle = !state
                .live_tasks
                .values()
                .any(|live| live.conversation_id == task.conversation_id);
            let over = state
                .docs
                .since_base
                .get(&doc_key(&reference))
                .copied()
                .unwrap_or(0)
                > Session::STICKY_BASE_BUDGET;
            (idle, over)
        };
        let reference_for_commit = reference;
        self.commit(
            Invoker::Kernel {
                conversation_id: None,
            },
            ctx.clone(),
            CommitOptions {
                docs: vec![reference_for_commit],
                closing: false,
            },
            |tx, _ctx| {
                let task = task.clone();
                async move {
                    tx.sticky_delete_task(task.conversation_id, task.id)?;
                    if idle || over {
                        tx.request_base(DocRef::Sticky {
                            conversation_id: task.conversation_id,
                        });
                    }
                    Ok(())
                }
                .boxed()
            },
        )
        .await?;
        if idle || over {
            let storage = self.storage.clone();
            let reference = reference_for_commit;
            self.on_line(
                |line_ctx| {
                    let storage = storage.clone();
                    async move {
                        storage
                            .truncate(&reference, without_abort_signal(line_ctx))
                            .await
                    }
                    .boxed()
                },
                ctx,
            )
            .await?;
        }
        Ok(())
    }

    /// Upstream `fork` (`session.ts:1418-1449`).
    pub async fn fork(
        self: &Arc<Self>,
        parent_id: Id,
        at: ParentAt,
        spec: ConversationSpec,
        ctx: Context,
    ) -> anyhow::Result<Id> {
        let inherited = match at {
            ParentAt::Start => None,
            ParentAt::Id(at) => {
                self.read(
                    move |storage, line_ctx| {
                        async move {
                            let entries = storage
                                .scan_entries(
                                    &EntryScan {
                                        conversation_id: parent_id,
                                        before: Some(at + 1),
                                        limit: 1,
                                        ..EntryScan::default()
                                    },
                                    line_ctx.clone(),
                                )
                                .await?;
                            if entries.first().map(|entry| entry.id) != Some(at) {
                                anyhow::bail!(
                                    "entry {at} is not visible from conversation {parent_id}"
                                );
                            }
                            storage.doc_as_of(parent_id, at, line_ctx).await
                        }
                        .boxed()
                    },
                    ctx.clone(),
                )
                .await?
            }
        };
        let mut raw_overrides = spec.rewindable.clone().unwrap_or_default();
        raw_overrides.shift_remove("plugins");
        let defaults = self.defaults();
        let overrides = defaults.validate_seed("rewindable", &raw_overrides)?;
        let rewindable = match inherited {
            None => overrides,
            Some(inherited) => {
                let mut merged = inherited.clone();
                merged.extend(overrides);
                if let Some(plugins) = inherited.get("plugins") {
                    merged.insert("plugins".to_owned(), plugins.clone());
                }
                merged
            }
        };
        let parent = ConversationParentSpec::Parent {
            conversation_id: parent_id,
            at,
        };
        let result = self
            .commit(
                Invoker::Kernel {
                    conversation_id: None,
                },
                ctx,
                CommitOptions::default(),
                |tx, _ctx| {
                    let mut spec = spec.clone();
                    spec.parent = Some(parent);
                    spec.rewindable = Some(rewindable.clone());
                    async move { tx.create_fork_conversation(&spec) }.boxed()
                },
            )
            .await?;
        Ok(result.value)
    }

    /// Upstream `close` (`session.ts:1451-1458`).
    pub async fn close(&self, ctx: Context) -> anyhow::Result<()> {
        let line_ctx = self.derive_line_ctx(&ctx)?;
        let _line = self.line.lock().await;
        {
            let state = self.state.lock().expect("session state");
            if state.closed {
                return Ok(());
            }
        }
        self.storage.close(without_abort_signal(line_ctx)).await?;
        self.state.lock().expect("session state").closed = true;
        owned_storages()
            .lock()
            .expect("owner registry")
            .remove(&storage_key(&self.storage));
        Ok(())
    }

    /// Upstream `loadedDocument` (`session.ts:1460-1462`).
    pub fn loaded_document(&self, reference: &DocRef) -> Option<JsonObject> {
        let state = self.state.lock().expect("session state");
        state.docs.loaded(&doc_key(reference))
    }

    fn apply_changes(&self, changes: &CommitChanges) {
        let mut state = self.state.lock().expect("session state");
        for task in &changes.tasks {
            if task.status == TaskStatus::Terminal {
                state.live_tasks.remove(&task.id);
                if !task.owns.is_empty() {
                    state.owner_task_cache.insert(task.id, task.clone());
                }
            } else {
                state.live_tasks.insert(task.id, task.clone());
            }
        }
        for conversation in &changes.conversations {
            state
                .conversation_records
                .insert(conversation.id, conversation.clone());
        }
    }

    fn assert_usable(&self) -> anyhow::Result<()> {
        let state = self.state.lock().expect("session state");
        if let Some(fault) = &state.fault {
            return Err(faulted(fault));
        }
        if state.closed {
            return Err(closed());
        }
        Ok(())
    }
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").finish_non_exhaustive()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        owned_storages()
            .lock()
            .expect("owner registry")
            .remove(&(Arc::as_ptr(&self.storage) as *const () as usize));
    }
}

// ---------------------------------------------------------------------------
// Transaction: one implementation, capability-checked per method
// ---------------------------------------------------------------------------

/// A touched document inside a transaction (`session.ts:281`).
struct TouchedDoc {
    reference: DocRef,
    tracker: Tracker,
}

/// Upstream `TxImpl` (`session.ts:279-1177`): one implementation,
/// capability-checked per method. Host operations are `pub`; core-only
/// operations are `pub(crate)` (the upstream capability surface; see the
/// module docs).
/// The transaction does not expose its backing Session as an authority escape hatch.
///
/// ```compile_fail
/// use pi_rust::agent_core::harness::pico3::session::Tx;
/// fn cannot_escalate(tx: &Tx) { let _ = tx.session(); }
/// ```
pub struct Tx {
    writes: Vec<Write>,
    touched: HashMap<String, TouchedDoc>,
    /// Overlays for direct reads: complete for the tables a closure reads
    /// after writing (`session.ts:283-288`).
    created_tasks: HashMap<Id, Task>,
    created_entries: HashMap<Id, Entry>,
    created_conversations: HashMap<Id, Conversation>,
    inputs_by_id: HashMap<Id, Input>,
    inputs_by_request: HashMap<String, Input>,
    changed_config: HashMap<Id, Vec<String>>,
    /// Domains written; scans over them reject (`session.ts:296-298`).
    wrote_entries: HashSet<Id>,
    wrote_tasks: bool,
    poisoned: Option<ReadAfterWrite>,
    surface_active: bool,
    /// Set while a terminal closure runs: the invoker's task counts as gone
    /// for busy() (`session.ts:301-302`).
    pub closing: bool,
    /// Upstream `readonly changes` (`session.ts:303`).
    pub changes: CommitChanges,
    invoker: Invoker,
    session: Arc<Session>,
    ctx: Context,
    membrane: Membrane,
}

/// The inbox placement decision (`types.ts:783-788`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundaryOutcome {
    pub triggers: Vec<Id>,
    pub terminated: bool,
}

/// The input resolution shapes of `resolveInputs` (`types.ts:777-782`).
#[derive(Debug, Clone, PartialEq)]
pub enum Resolution {
    Done {
        answer: Id,
    },
    Unanswered {
        reason: String,
        detail: Option<String>,
    },
}

/// `createTask`'s option object (`types.ts:734-738`).
#[derive(Debug, Clone, Default)]
pub struct CreateTaskOptions {
    pub conversation_id: Option<Id>,
    pub background: bool,
    pub after: Vec<Id>,
}

/// The core-only surface (`pub(crate)`) is consumed by the scheduler
/// (M3b Task 9) and by the oracle tests; unused members stay for that
/// landing.
#[allow(dead_code)]
impl Tx {
    fn new(invoker: &Invoker, session: Arc<Session>, ctx: Context) -> Tx {
        Tx {
            writes: Vec::new(),
            touched: HashMap::new(),
            created_tasks: HashMap::new(),
            created_entries: HashMap::new(),
            created_conversations: HashMap::new(),
            inputs_by_id: HashMap::new(),
            inputs_by_request: HashMap::new(),
            changed_config: HashMap::new(),
            wrote_entries: HashSet::new(),
            wrote_tasks: false,
            poisoned: None,
            surface_active: true,
            closing: false,
            changes: CommitChanges::default(),
            invoker: invoker.clone(),
            session,
            ctx,
            membrane: Membrane::new("tx"),
        }
    }

    /// Upstream `closeSurface` (`session.ts:359-361`).
    pub fn close_surface(&mut self) {
        self.surface_active = false;
    }

    /// Upstream `assertSurfaceActive` (`session.ts:362-364`).
    fn assert_surface_active(&self) -> anyhow::Result<()> {
        if !self.surface_active {
            anyhow::bail!("transaction used outside its callback");
        }
        self.membrane.check().map_err(anyhow::Error::msg)
    }

    // --- capability & scope ------------------------------------------------

    /// Upstream `get core` (`session.ts:368-370`).
    fn core(&self) -> bool {
        self.invoker.is_core()
    }

    fn assert_core(&self, what: &str) -> anyhow::Result<()> {
        if !self.core() {
            return Err(forbidden(format!("{what}: core turn machinery only")));
        }
        Ok(())
    }

    fn assert_not_host(&self, what: &str) -> anyhow::Result<()> {
        if self.invoker.task_id().is_none() {
            return Err(forbidden(format!("{what} outside a task")));
        }
        Ok(())
    }

    /// A task may touch its own conversation and the subtree it owns; the
    /// host and core may touch anything (`session.ts:377-384`).
    fn in_scope(&self, conversation_id: Id) -> anyhow::Result<bool> {
        let Invoker::Task {
            conversation_id: own,
            id,
            core,
            ..
        } = &self.invoker
        else {
            return Ok(true);
        };
        if *core {
            return Ok(true);
        }
        if conversation_id == *own || self.created_conversations.contains_key(&conversation_id) {
            return Ok(true);
        }
        let owns: Vec<Id> = {
            let state = self.session.state.lock().expect("session state");
            self.created_tasks
                .get(id)
                .or_else(|| state.live_tasks.get(id))
                .map(|task| task.owns.clone())
                .unwrap_or_default()
        };
        for root in &owns {
            if self
                .session
                .index()
                .subtree(*root)
                .contains(&conversation_id)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn assert_scope(&self, conversation_id: Id, what: &str) -> anyhow::Result<()> {
        if !self.in_scope(conversation_id)? {
            return Err(forbidden(format!(
                "{what}: conversation {conversation_id} is outside this task's subtree"
            )));
        }
        Ok(())
    }

    /// Upstream `assertEntryScope` (`session.ts:389-405`).
    fn assert_entry_scope(&self, entry: &Entry, what: &str) -> anyhow::Result<()> {
        if self.in_scope(entry.conversation_id)? {
            return Ok(());
        }
        let Invoker::Task {
            conversation_id: own,
            id,
            core,
            ..
        } = &self.invoker
        else {
            return Err(forbidden(format!(
                "{what}: entry {} is outside this task's subtree",
                entry.id
            )));
        };
        if *core {
            return Err(forbidden(format!(
                "{what}: entry {} is outside this task's subtree",
                entry.id
            )));
        }
        let mut candidates: HashSet<Id> = HashSet::from([*own]);
        candidates.extend(self.created_conversations.keys());
        // Collect owns and the records snapshot first; `subtree` locks the
        // session state itself, so it must run outside the guard.
        let (owns, records): (Vec<Id>, std::collections::HashMap<Id, Conversation>) = {
            let state = self.session.state.lock().expect("session state");
            let owns = self
                .created_tasks
                .get(id)
                .or_else(|| state.live_tasks.get(id))
                .map(|task| task.owns.clone())
                .unwrap_or_default();
            (owns, state.conversation_records.clone())
        };
        for root in &owns {
            candidates.extend(self.session.index().subtree(*root));
        }
        for candidate in candidates {
            let mut conversation = self
                .created_conversations
                .get(&candidate)
                .cloned()
                .or_else(|| records.get(&candidate).cloned());
            while let Some(current) = conversation {
                let Some(parent) = current.parent else {
                    break;
                };
                if parent.conversation_id == entry.conversation_id && entry.id <= parent.at {
                    return Ok(());
                }
                conversation = records.get(&parent.conversation_id).cloned();
            }
        }
        Err(forbidden(format!(
            "{what}: entry {} is not visible from this task's subtree",
            entry.id
        )))
    }

    /// Upstream `poison` (`session.ts:406-409`): the first poison wins. The
    /// typed error is kept so `finish` rethrows it with its type intact.
    fn poison(&mut self, error: ReadAfterWrite) -> anyhow::Error {
        if self.poisoned.is_none() {
            self.poisoned = Some(error.clone());
        }
        anyhow::Error::new(error)
    }

    fn assert_no_entry_writes(&mut self, conversation_id: Id, read: &str) -> anyhow::Result<()> {
        if self.wrote_entries.contains(&conversation_id) {
            let error = ReadAfterWrite::new(read, "entry append");
            return Err(self.poison(error));
        }
        Ok(())
    }

    fn assert_no_task_writes(&mut self, read: &str) -> anyhow::Result<()> {
        if self.wrote_tasks {
            let error = ReadAfterWrite::new(read, "task write");
            return Err(self.poison(error));
        }
        Ok(())
    }

    /// Upstream `get poison_` (`session.ts:416-418`).
    pub fn poisoned(&self) -> Option<&ReadAfterWrite> {
        self.poisoned.as_ref()
    }

    // --- documents ----------------------------------------------------------

    /// Upstream `doc` (`session.ts:526-536`) plus preload-on-touch: upstream
    /// shares tracker objects with the cache, the port moves the cached
    /// tracker into `touched`, which preserves the observable behavior under
    /// the line lock (see the module docs).
    fn doc_mut(&mut self, reference: DocRef) -> anyhow::Result<&mut Tracker> {
        self.assert_surface_active()?;
        let key = doc_key(&reference);
        if !self.touched.contains_key(&key) {
            let mut state = self.session.state.lock().expect("session state");
            if let Some(tracker) = state.docs.cache.remove(&key) {
                self.touched
                    .insert(key.clone(), TouchedDoc { reference, tracker });
            } else {
                drop(state);
                anyhow::bail!("document {key} not loaded; pass it in commit({{ docs }})");
            }
        }
        Ok(&mut self.touched.get_mut(&key).expect("checked above").tracker)
    }

    fn doc_state(&mut self, reference: DocRef) -> anyhow::Result<Value> {
        let tracker = self.doc_mut(reference)?;
        Ok(tracker.state().clone())
    }

    /// Run `f` against a document's tracked state, marking it dirty.
    fn with_doc<R>(
        &mut self,
        reference: DocRef,
        f: impl FnOnce(&mut Value) -> anyhow::Result<R>,
    ) -> anyhow::Result<R> {
        let tracker = self.doc_mut(reference)?;
        f(tracker.state_mut())
    }

    /// Preload document refs into the transaction (`session.ts:540-546`).
    pub(crate) async fn preload(&mut self, refs: Vec<DocRef>, ctx: Context) -> anyhow::Result<()> {
        for reference in refs {
            let key = doc_key(&reference);
            if self.touched.contains_key(&key) {
                continue;
            }
            {
                let mut state = self.session.state.lock().expect("session state");
                if let Some(tracker) = state.docs.cache.remove(&key) {
                    self.touched
                        .insert(key.clone(), TouchedDoc { reference, tracker });
                    continue;
                }
            }
            let stored = self.session.storage.doc(&reference, ctx.clone()).await?;
            let Some(mut stored) = stored else {
                anyhow::bail!("document {key} does not exist");
            };
            if let DocRef::Rewindable { .. } | DocRef::Sticky { .. } = &reference {
                self.session
                    .defaults()
                    .fill(doc_name(&reference), &mut stored);
            }
            let mut tracker = track(Value::Object(stored));
            // Consume the synthetic first flush; never persisted
            // (`session.ts:201-203`).
            let _ = tracker.flush();
            self.touched.insert(key, TouchedDoc { reference, tracker });
        }
        Ok(())
    }

    /// Upstream `requestBase` on the tx surface (used by `retire`).
    pub(crate) fn request_base(&mut self, reference: DocRef) {
        let key = doc_key(&reference);
        if let Some(touched) = self.touched.get_mut(&key) {
            touched.tracker.rebase();
        } else {
            let mut state = self.session.state.lock().expect("session state");
            state.docs.request_base(&key);
        }
    }

    /// Delete a task's live slot from its conversation's sticky doc (the
    /// `retire` commit body, `session.ts:1408-1411`).
    pub(crate) fn sticky_delete_task(
        &mut self,
        conversation_id: Id,
        task_id: Id,
    ) -> anyhow::Result<()> {
        self.with_doc(DocRef::Sticky { conversation_id }, |state| {
            if let Some(tasks) = state.get_mut("tasks").and_then(Value::as_object_mut) {
                tasks.shift_remove(&task_id.to_string());
            }
            Ok(())
        })
    }

    /// Move every touched tracker back into the session cache (the port's
    /// equivalent of upstream's shared references staying in `docs`).
    fn return_touched_to_cache(&mut self) {
        let mut state = self.session.state.lock().expect("session state");
        for (key, touched) in self.touched.drain() {
            state.docs.cache.insert(key, touched.tracker);
        }
    }

    /// Upstream `evictTouched` (`session.ts:1174-1176`).
    fn evict_touched(&mut self) {
        let mut state = self.session.state.lock().expect("session state");
        for key in self.touched.keys() {
            state.docs.evict(key);
        }
        self.touched.clear();
    }

    // --- reads (direct reads see this transaction's writes) -------------------

    /// Upstream `conversation` (`session.ts:422-425`).
    pub async fn conversation(&self, id: Id) -> anyhow::Result<Option<Conversation>> {
        self.assert_scope(id, "conversation")?;
        if let Some(created) = self.created_conversations.get(&id) {
            return Ok(Some(created.clone()));
        }
        self.session
            .storage
            .conversation(id, self.ctx.clone())
            .await
    }

    /// Upstream `entry(id)` (`session.ts:426-433`).
    pub async fn entry(&mut self, id: Id) -> anyhow::Result<Option<Entry>> {
        let entry = match self.created_entries.get(&id) {
            Some(created) => Some(created.clone()),
            None => self
                .session
                .storage
                .entries(&[id], self.ctx.clone())
                .await?
                .remove(&id),
        };
        if let Some(entry) = &entry {
            self.assert_entry_scope(entry, "entry")?;
        }
        Ok(entry)
    }

    /// Upstream `entries(ids)` (`session.ts:434-445`).
    pub async fn entries(&mut self, ids: &[Id]) -> anyhow::Result<HashMap<Id, Entry>> {
        let stored_ids: Vec<Id> = ids
            .iter()
            .copied()
            .filter(|id| !self.created_entries.contains_key(id))
            .collect();
        let mut out = self
            .session
            .storage
            .entries(&stored_ids, self.ctx.clone())
            .await?;
        for id in ids {
            if let Some(created) = self.created_entries.get(id) {
                out.insert(*id, created.clone());
            }
        }
        for entry in out.values() {
            self.assert_entry_scope(entry, "entries")?;
        }
        Ok(out)
    }

    /// Upstream `newestEntry` (`session.ts:446-457`).
    pub async fn newest_entry(
        &mut self,
        conversation_id: Id,
        kind: Option<&str>,
        with_head: bool,
    ) -> anyhow::Result<Option<Entry>> {
        self.assert_scope(conversation_id, "newestEntry")?;
        self.assert_no_entry_writes(conversation_id, "newestEntry")?;
        let scan = EntryScan {
            conversation_id,
            kind: kind.map(str::to_owned),
            with_head,
            limit: 1,
            ..EntryScan::default()
        };
        self.session
            .storage
            .scan_entries(&scan, self.ctx.clone())
            .await
            .map(|mut entries| entries.drain(..).next())
    }

    /// Upstream `scanEntries` (`session.ts:458-462`).
    pub async fn scan_entries(&mut self, scan: &EntryScan) -> anyhow::Result<Vec<Entry>> {
        self.assert_scope(scan.conversation_id, "scanEntries")?;
        self.assert_no_entry_writes(scan.conversation_id, "scanEntries")?;
        self.session
            .storage
            .scan_entries(scan, self.ctx.clone())
            .await
    }

    /// Upstream `context` (`session.ts:463-467`).
    pub async fn context(
        &mut self,
        conversation_id: Id,
        at: Option<Id>,
    ) -> anyhow::Result<ContextView> {
        self.assert_scope(conversation_id, "context")?;
        self.assert_no_entry_writes(conversation_id, "context")?;
        derive_context(
            self.session.storage.as_ref(),
            conversation_id,
            at,
            self.ctx.clone(),
        )
        .await
    }

    /// Upstream `task(id)` (`session.ts:468-475`).
    pub async fn task(&mut self, id: Id) -> anyhow::Result<Option<Task>> {
        {
            let state = self.session.state.lock().expect("session state");
            if let Some(task) = self
                .created_tasks
                .get(&id)
                .cloned()
                .or_else(|| state.live_tasks.get(&id).cloned())
            {
                self.assert_scope(task.conversation_id, "task")?;
                return Ok(Some(task));
            }
        }
        let task = self.session.storage.task(id, self.ctx.clone()).await?;
        if let Some(task) = &task {
            self.assert_scope(task.conversation_id, "task")?;
        }
        Ok(task)
    }

    /// Upstream `tasks(scan)` (`session.ts:476-489`).
    pub async fn tasks(&mut self, scan: &TaskScan) -> anyhow::Result<Vec<Task>> {
        self.assert_no_task_writes("tasks")?;
        if let Some(conversation_id) = scan.conversation_id {
            self.assert_scope(conversation_id, "tasks")?;
        }
        let mut rows = self
            .session
            .storage
            .scan_tasks(scan, self.ctx.clone())
            .await?;
        let seen: HashSet<Id> = rows.iter().map(|task| task.id).collect();
        {
            let state = self.session.state.lock().expect("session state");
            for task in state.live_tasks.values() {
                if seen.contains(&task.id) {
                    continue;
                }
                if let Some(conversation_id) = scan.conversation_id {
                    if task.conversation_id != conversation_id {
                        continue;
                    }
                }
                if let Some(kind) = &scan.kind {
                    if &task.kind != kind {
                        continue;
                    }
                }
                rows.push(task.clone());
            }
        }
        if let Some(statuses) = &scan.status {
            rows.retain(|task| statuses.contains(&task.status));
        }
        Ok(rows)
    }

    /// Upstream `input(id)` (`session.ts:490-494`).
    pub async fn input(&mut self, id: Id) -> anyhow::Result<Option<Input>> {
        let input = match self.inputs_by_id.get(&id) {
            Some(overlay) => Some(overlay.clone()),
            None => self.session.storage.input(id, self.ctx.clone()).await?,
        };
        if let Some(input) = &input {
            self.assert_scope(input.conversation_id, "input")?;
        }
        Ok(input)
    }

    /// Upstream `inputByRequest` (`session.ts:495-501`).
    async fn input_by_request(
        &mut self,
        conversation_id: Id,
        request_id: &str,
    ) -> anyhow::Result<Option<Input>> {
        self.assert_scope(conversation_id, "inputByRequest")?;
        let key = format!("{conversation_id}:{request_id}");
        if let Some(overlay) = self.inputs_by_request.get(&key) {
            return Ok(Some(overlay.clone()));
        }
        self.session
            .storage
            .input_by_request(conversation_id, request_id, self.ctx.clone())
            .await
    }

    /// Upstream `rewindableAsOf` (`session.ts:502-505`).
    pub async fn rewindable_as_of(
        &mut self,
        conversation_id: Id,
        at: Id,
    ) -> anyhow::Result<Option<JsonObject>> {
        self.assert_scope(conversation_id, "rewindableAsOf")?;
        self.session
            .storage
            .doc_as_of(conversation_id, at, self.ctx.clone())
            .await
    }

    /// Upstream `snapshot` (`session.ts:507-522`): plain copies; document
    /// proxies cannot escape a transaction (the port clones outright).
    pub fn snapshot(&mut self, reference: DocRef) -> anyhow::Result<JsonObject> {
        if !matches!(reference, DocRef::Session) {
            let conversation_id = match reference {
                DocRef::Rewindable { conversation_id } | DocRef::Sticky { conversation_id } => {
                    conversation_id
                }
                DocRef::Session => unreachable!("checked above"),
            };
            self.assert_scope(conversation_id, "snapshot")?;
        }
        let mut value = match self.doc_state(reference)? {
            Value::Object(object) => object,
            _ => JsonObject::new(),
        };
        if let DocRef::Rewindable { .. } | DocRef::Sticky { .. } = &reference {
            self.session
                .defaults()
                .fill(doc_name(&reference), &mut value);
            if let Invoker::Task { kind, .. } = &self.invoker {
                if let Some(declared) = kind.config().map(|config| match doc_name(&reference) {
                    "rewindable" => &config.rewindable,
                    _ => &config.sticky,
                }) {
                    for (key, fallback) in declared {
                        // `fallback !== undefined` (`session.ts:518`): a
                        // declared null is a value and IS applied; only
                        // absence (the key not being declared) skips.
                        if !value.contains_key(key) {
                            value.insert(key.clone(), plain(fallback));
                        }
                    }
                }
            }
        }
        Ok(value)
    }

    // --- core document views -------------------------------------------------

    /// Upstream `rewindable` (`session.ts:548-551`): the live rewindable
    /// document is a core-only view; the port exposes targeted operations.
    pub(crate) fn rewindable_set(
        &mut self,
        conversation_id: Id,
        key: &str,
        value: Value,
    ) -> anyhow::Result<()> {
        self.assert_core("rewindable document")?;
        self.with_doc(DocRef::Rewindable { conversation_id }, |state| {
            ensure_object(state).insert(key.to_owned(), value);
            Ok(())
        })
    }

    /// Read one key off a core document view (test/access surface for
    /// [`Tx::rewindable_set`]'s family).
    #[allow(dead_code)]
    pub(crate) fn rewindable_get(
        &mut self,
        conversation_id: Id,
        key: &str,
    ) -> anyhow::Result<Option<Value>> {
        self.assert_core("rewindable document")?;
        let state = self.doc_state(DocRef::Rewindable { conversation_id })?;
        Ok(state.get(key).cloned())
    }

    /// Upstream `sticky(conversationId)` targeted write.
    pub(crate) fn sticky_set(
        &mut self,
        conversation_id: Id,
        key: &str,
        value: Value,
    ) -> anyhow::Result<()> {
        self.assert_core("sticky document")?;
        self.with_doc(DocRef::Sticky { conversation_id }, |state| {
            ensure_object(state).insert(key.to_owned(), value);
            Ok(())
        })
    }

    /// Upstream `sticky(conversationId)` targeted read.
    #[allow(dead_code)]
    pub(crate) fn sticky_get(
        &mut self,
        conversation_id: Id,
        key: &str,
    ) -> anyhow::Result<Option<Value>> {
        self.assert_core("sticky document")?;
        let state = self.doc_state(DocRef::Sticky { conversation_id })?;
        Ok(state.get(key).cloned())
    }

    /// Upstream `session()` targeted write on the session document.
    pub(crate) fn session_doc_set(&mut self, key: &str, value: Value) -> anyhow::Result<()> {
        self.assert_core("session document")?;
        self.with_doc(DocRef::Session, |state| {
            ensure_object(state).insert(key.to_owned(), value);
            Ok(())
        })
    }

    /// Delete a key from a core document (e.g. config reset).
    fn doc_delete(&mut self, reference: DocRef, key: &str) -> anyhow::Result<()> {
        self.with_doc(reference, |state| {
            if let Some(object) = state.as_object_mut() {
                object.shift_remove(key);
            }
            Ok(())
        })
    }

    // --- namespaces -----------------------------------------------------------

    /// Upstream `plugins` (`session.ts:565-591`): the transaction-scoped
    /// namespace view.
    pub fn plugins(&mut self, namespace: &Namespace) -> anyhow::Result<PluginsView<'_>> {
        let (routes, defaults) = {
            let registrations = self.session.namespaces.read().expect("namespaces");
            let Some(registration) = registrations.get(&namespace.id) else {
                return Err(forbidden(format!(
                    "namespace \"{}\" is stale",
                    namespace.id
                )));
            };
            if registration.generation != namespace.generation {
                return Err(forbidden(format!(
                    "namespace \"{}\" is stale",
                    namespace.id
                )));
            }
            (registration.routes.clone(), registration.defaults.clone())
        };
        // `invocationConversationId("plugins(ns)")` (`session.ts:570`):
        // invokers with no bound conversation reject here, before any
        // document is touched.
        let conversation_id =
            self.invocation_conversation_id(&format!("plugins({})", namespace.id))?;
        Ok(PluginsView {
            tx: self,
            namespace_id: namespace.id.clone(),
            routes,
            defaults,
            conversation_id,
        })
    }

    /// Upstream `emit(namespace, name, data)` (`session.ts:593-610`).
    pub fn emit_plugin(
        &mut self,
        namespace: &Namespace,
        name: &str,
        data: Value,
    ) -> anyhow::Result<()> {
        {
            let registrations = self.session.namespaces.read().expect("namespaces");
            let stale = match registrations.get(&namespace.id) {
                Some(registration) => registration.generation != namespace.generation,
                None => true,
            };
            if stale {
                return Err(forbidden(format!(
                    "namespace \"{}\" is stale",
                    namespace.id
                )));
            }
        }
        let valid_name = name
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic())
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
        if !(valid_name) {
            anyhow::bail!("invalid plugin event");
        }
        let conversation_id = self.invocation_conversation_id("emit")?;
        self.changes.events.push((
            conversation_id,
            ViewEvent::Plugin {
                namespace: namespace.id.clone(),
                name: name.to_owned(),
                data: plain(&data),
            },
        ));
        Ok(())
    }

    /// Upstream `emit(event)` (`session.ts:608-610`): core events.
    pub(crate) fn emit_event(&mut self, event: ViewEvent) -> anyhow::Result<()> {
        self.assert_core("core event")?;
        let conversation_id = self.invocation_conversation_id("emit")?;
        self.changes.events.push((conversation_id, event));
        Ok(())
    }

    fn invocation_conversation_id(&self, what: &str) -> anyhow::Result<Id> {
        let Some(conversation_id) = self.invoker.conversation_id() else {
            return Err(forbidden(format!(
                "{what}: no conversation is bound to this transaction"
            )));
        };
        self.assert_scope(conversation_id, what)?;
        Ok(conversation_id)
    }

    // --- config -----------------------------------------------------------------

    /// Upstream `config(conversationId).get` (`session.ts:619-641`).
    pub fn config_get(&mut self, conversation_id: Id, key: &str) -> anyhow::Result<Value> {
        self.assert_scope(conversation_id, "config")?;
        self.assert_surface_active()?;
        let (doc, fallback) = self.config_definition(conversation_id, key)?;
        let document = self.doc_state(DocRef::from_name(&doc, conversation_id))?;
        match document.get(key) {
            Some(value) => Ok(plain(value)),
            None => Ok(fallback.unwrap_or(Value::Null)),
        }
    }

    /// Upstream `config(...).set` (`session.ts:643-652`).
    pub fn config_set(
        &mut self,
        conversation_id: Id,
        key: &str,
        value: Value,
    ) -> anyhow::Result<()> {
        self.assert_scope(conversation_id, "config")?;
        self.assert_surface_active()?;
        self.assert_config_writable(conversation_id, key)?;
        let (doc, _) = self.config_definition(conversation_id, key)?;
        let defaults = self.session.defaults();
        if !defaults.validate(key, &value) {
            anyhow::bail!("invalid config value for \"{key}\"");
        }
        let reference = DocRef::from_name(&doc, conversation_id);
        let value = plain(&value);
        self.with_doc(reference, move |state| {
            ensure_object(state).insert(key.to_owned(), value);
            Ok(())
        })?;
        self.mark_changed_config(conversation_id, key);
        Ok(())
    }

    /// Upstream `config(...).reset` (`session.ts:653-661`).
    pub fn config_reset(&mut self, conversation_id: Id, key: &str) -> anyhow::Result<()> {
        self.assert_scope(conversation_id, "config")?;
        self.assert_surface_active()?;
        self.assert_config_writable(conversation_id, key)?;
        let (doc, _) = self.config_definition(conversation_id, key)?;
        self.doc_delete(DocRef::from_name(&doc, conversation_id), key)?;
        self.mark_changed_config(conversation_id, key);
        Ok(())
    }

    fn assert_config_writable(&self, conversation_id: Id, key: &str) -> anyhow::Result<()> {
        let _ = conversation_id;
        if let Invoker::Task { core: false, .. } = &self.invoker {
            return Err(forbidden(format!(
                "config({key}): ordinary tasks cannot write config"
            )));
        }
        Ok(())
    }

    fn mark_changed_config(&mut self, conversation_id: Id, key: &str) {
        let keys = self.changed_config.entry(conversation_id).or_default();
        if !keys.iter().any(|existing| existing == key) {
            keys.push(key.to_owned());
        }
    }

    /// Upstream `definition` (`session.ts:621-631`): the declaring document
    /// and fallback for one config key.
    fn config_definition(
        &self,
        conversation_id: Id,
        key: &str,
    ) -> anyhow::Result<(String, Option<Value>)> {
        let _ = conversation_id;
        if let Invoker::Task { kind, .. } = &self.invoker {
            if let Some(config) = kind.config() {
                if config.rewindable.contains_key(key)
                    || config.declared_absent.rewindable.iter().any(|k| k == key)
                {
                    return Ok(("rewindable".to_owned(), config.rewindable.get(key).cloned()));
                }
                if config.sticky.contains_key(key)
                    || config.declared_absent.sticky.iter().any(|k| k == key)
                {
                    return Ok(("sticky".to_owned(), config.sticky.get(key).cloned()));
                }
            }
        }
        let defaults = self.session.defaults();
        match defaults.route.get(key) {
            Some(doc) => {
                let fallback = match doc.as_str() {
                    "rewindable" => defaults.rewindable.get(key).cloned(),
                    _ => defaults.sticky.get(key).cloned(),
                };
                Ok((doc.clone(), fallback))
            }
            None => anyhow::bail!("unknown config key \"{key}\""),
        }
    }

    // --- slots -------------------------------------------------------------------

    /// Upstream `slot(ref)` (`session.ts:665-674`), get form: the slot value
    /// is materialized (defaulting via the kind's initializer) and returned
    /// as a plain copy.
    pub(crate) fn slot_get(&mut self, reference: &TaskRef) -> anyhow::Result<Value> {
        self.assert_not_host("slot")?;
        let Invoker::Task {
            id: invoker_id,
            core,
            ..
        } = &self.invoker
        else {
            unreachable!("assert_not_host checked above")
        };
        let task = {
            let state = self.session.state.lock().expect("session state");
            self.created_tasks
                .get(&reference.id)
                .cloned()
                .or_else(|| state.live_tasks.get(&reference.id).cloned())
        };
        let Some(task) = task else {
            anyhow::bail!("task {} is not live", reference.id);
        };
        if !*core && reference.id != *invoker_id {
            return Err(forbidden("slot: another task's slot"));
        }
        let conversation_id = task.conversation_id;
        let slot_default = reference.kind.slot_init(&task.input);
        self.with_doc(DocRef::Sticky { conversation_id }, |state| {
            let tasks = ensure_object_entry(ensure_object(state), "tasks");
            let entry = tasks
                .entry(reference.id.to_string())
                .or_insert_with(|| Value::Object(slot_default.clone()));
            if !entry.is_object() {
                *entry = Value::Object(slot_default.clone());
            }
            Ok(())
        })?;
        let state = self.doc_state(DocRef::Sticky { conversation_id })?;
        Ok(state
            .get("tasks")
            .and_then(|tasks| tasks.get(reference.id.to_string()))
            .cloned()
            .unwrap_or(Value::Null))
    }

    /// Upstream `slot(ref)` write form: mutate the live slot in place.
    pub(crate) fn slot_update(
        &mut self,
        reference: &TaskRef,
        update: impl FnOnce(&mut Value),
    ) -> anyhow::Result<()> {
        let value = self.slot_get(reference)?;
        let mut value = value;
        update(&mut value);
        let conversation_id = {
            let state = self.session.state.lock().expect("session state");
            self.created_tasks
                .get(&reference.id)
                .or_else(|| state.live_tasks.get(&reference.id))
                .map(|task| task.conversation_id)
        };
        let Some(conversation_id) = conversation_id else {
            anyhow::bail!("task {} is not live", reference.id);
        };
        let id = reference.id;
        self.with_doc(DocRef::Sticky { conversation_id }, move |state| {
            if let Some(tasks) = state.get_mut("tasks").and_then(Value::as_object_mut) {
                tasks.insert(id.to_string(), value);
            }
            Ok(())
        })
    }

    /// Upstream `toolSlot(task)` (`session.ts:675-682`).
    pub(crate) fn tool_slot(&mut self, conversation_id: Id, index: usize) -> anyhow::Result<Value> {
        self.assert_core("toolSlot")?;
        let state = self.doc_state(DocRef::Sticky { conversation_id })?;
        let slot = state
            .get("turn")
            .and_then(|turn| turn.get("tools"))
            .and_then(Value::as_array)
            .and_then(|tools| tools.get(index));
        match slot {
            Some(slot) => Ok(slot.clone()),
            None => anyhow::bail!("no tool slot at index {index}"),
        }
    }

    // --- writes -----------------------------------------------------------------

    /// Upstream `appendEntry` (`session.ts:686-719`).
    pub(crate) fn append_entry(
        &mut self,
        conversation_id: Id,
        entry: NewEntry,
    ) -> anyhow::Result<Id> {
        self.assert_core("appendEntry")?;
        self.append_entry_internal(conversation_id, entry)
    }

    fn append_entry_internal(
        &mut self,
        conversation_id: Id,
        entry: NewEntry,
    ) -> anyhow::Result<Id> {
        if let Invoker::Task { id, .. } = &self.invoker {
            let live = self
                .session
                .state
                .lock()
                .expect("session state")
                .live_tasks
                .contains_key(id);
            if !live && !self.closing {
                return Err(forbidden("appendEntry from a task that is not live"));
            }
        }
        let id = self.session.storage.mint_id();
        let head = match entry.head {
            Some(Head::Self_) => Some(id),
            Some(Head::Id(head_id)) => Some(head_id),
            None => None,
        };
        let NewEntry {
            kind,
            model,
            data,
            edits,
            ..
        } = entry;
        let mut record = Entry {
            id,
            conversation_id,
            kind,
            model,
            data,
            head,
            edits,
            by_task_id: None,
        };
        if let Invoker::Task { id, .. } = &self.invoker {
            record.by_task_id = Some(*id);
        }
        validate_entry(&record)?;
        self.writes.push(Write::Entry {
            entry: record.clone(),
        });
        self.changes.entries.push(record.clone());
        if let Some(head) = record.head {
            self.changes.events.push((
                conversation_id,
                ViewEvent::HeadMoved {
                    entry: record.clone(),
                },
            ));
            let _ = head;
        }
        self.changes.events.push((
            conversation_id,
            ViewEvent::EntryAdded {
                entry: record.clone(),
            },
        ));
        self.created_entries.insert(id, record);
        self.wrote_entries.insert(conversation_id);
        Ok(id)
    }

    /// Upstream `write` (`session.ts:721-750`): passive entry — queued when
    /// busy, appended when idle. Returns the input id.
    pub async fn write(&mut self, conversation_id: Id, entry: NewEntry) -> anyhow::Result<Id> {
        self.assert_scope(conversation_id, "write")?;
        if !self.core() {
            if entry.head.is_some() {
                return Err(forbidden("write: head entries are core only"));
            }
            if entry.edits.is_some() {
                return Err(forbidden("write: edits are core only"));
            }
            if entry.kind.starts_with("pi.") && entry.kind != "pi.notice" {
                return Err(forbidden(format!("write: kind {} is reserved", entry.kind)));
            }
            let single_user_model = entry.model.as_ref().map(Vec::len) == Some(1)
                && entry
                    .model
                    .as_ref()
                    .and_then(|model| model.first())
                    .and_then(|message| message.get("role").cloned())
                    == Some(Value::String("user".to_owned()));
            if entry.kind == "pi.notice" && !single_user_model {
                return Err(forbidden(
                    "write: pi.notice requires exactly one user model message",
                ));
            }
        }
        if self.busy(conversation_id)? {
            let id = self.session.storage.mint_id();
            let queued_entry = serde_json::to_value(&entry)?;
            self.with_doc(DocRef::Sticky { conversation_id }, move |state| {
                let inbox = ensure_key_array(state, "inbox");
                inbox.push(Value::Object(JsonObject::from_iter([
                    ("id".to_owned(), number(id)),
                    ("mode".to_owned(), Value::String("write".to_owned())),
                    ("entry".to_owned(), queued_entry),
                ])));
                Ok(())
            })?;
            self.put_input(Input {
                id,
                conversation_id,
                request_id: None,
                status: "queued".to_owned(),
                entry: None,
                answer: None,
                reason: None,
                detail: None,
            });
            self.changes.events.push((
                conversation_id,
                ViewEvent::InputQueued {
                    input: id,
                    mode: "write".to_owned(),
                },
            ));
            return Ok(id);
        }
        let id = self.session.storage.mint_id();
        let placed = self.append_entry_internal(conversation_id, entry)?;
        self.put_input(Input {
            id,
            conversation_id,
            request_id: None,
            status: "done".to_owned(),
            entry: Some(placed),
            answer: None,
            reason: None,
            detail: None,
        });
        Ok(id)
    }

    /// Upstream `checkpoint` (`session.ts:752-759`).
    pub(crate) fn checkpoint(&mut self, value: Value) -> anyhow::Result<()> {
        self.assert_not_host("checkpoint")?;
        let Invoker::Task {
            id,
            mode,
            conversation_id,
            ..
        } = &self.invoker
        else {
            unreachable!("assert_not_host checked above")
        };
        if *mode == InvocationMode::Abort {
            return Err(forbidden("checkpoint from an abort invocation"));
        }
        let _ = conversation_id;
        let current = {
            let state = self.session.state.lock().expect("session state");
            self.created_tasks
                .get(id)
                .cloned()
                .or_else(|| state.live_tasks.get(id).cloned())
        };
        let Some(mut current) = current else {
            anyhow::bail!("task not live");
        };
        current.checkpoint = Some(plain(&value).as_object().cloned().unwrap_or_default());
        self.set_task_internal(current)
    }

    /// Upstream `setTask` (`session.ts:761-764`): kernel-internal; replace a
    /// task's mutable fields, persisting only what changed.
    pub(crate) fn set_task(&mut self, task: Task) -> anyhow::Result<()> {
        self.assert_core("setTask")?;
        self.set_task_internal(task)
    }

    /// Upstream `setTaskInternalForControl` (`session.ts:765-767`): the
    /// terminal-closure control path.
    pub(crate) fn set_task_for_control(&mut self, task: Task) -> anyhow::Result<()> {
        self.set_task_internal(task)
    }

    fn set_task_internal(&mut self, task: Task) -> anyhow::Result<()> {
        let mut task = plain_record(&task);
        let prev = {
            let state = self.session.state.lock().expect("session state");
            self.created_tasks
                .get(&task.id)
                .cloned()
                .or_else(|| state.live_tasks.get(&task.id).cloned())
        };
        let mut patch = TaskPatch::new(task.id);
        // Upstream compares `JSON.stringify(prev?.[key]) ===
        // JSON.stringify(task[key])` (`session.ts:771-775`): absent (None)
        // and stored null (Some(Null)) are different values, and
        // absent-vs-absent compares equal so the key is skipped entirely.
        let prev_status = prev.as_ref().map(|p| json!(p.status));
        let task_status = Some(json!(task.status));
        if prev_status != task_status {
            patch.status = Some(task.status);
        }
        let prev_checkpoint = prev
            .as_ref()
            .and_then(|p| p.checkpoint.as_ref())
            .map(|c| json!(c));
        let task_checkpoint = task.checkpoint.as_ref().map(|c| json!(c));
        if prev_checkpoint != task_checkpoint {
            patch.checkpoint = Some(task.checkpoint.clone());
        }
        if prev.as_ref().and_then(|p| p.abort) != task.abort {
            patch.abort = task.abort;
        }
        let prev_outcome = prev
            .as_ref()
            .and_then(|p| p.outcome.as_ref())
            .map(|o| json!(o));
        let task_outcome = task.outcome.as_ref().map(|o| json!(o));
        if prev_outcome != task_outcome {
            patch.outcome = task.outcome.clone();
        }
        let prev_owns = prev.as_ref().map(|p| json!(p.owns));
        let task_owns = Some(json!(task.owns));
        if prev_owns != task_owns {
            patch.owns = Some(task.owns.clone());
        }
        if task.status == TaskStatus::Terminal {
            patch.status = Some(TaskStatus::Terminal);
            patch.checkpoint = Some(None);
            patch.owns = Some(task.owns.clone());
            patch.outcome = task.outcome.clone();
            if task.abort == Some(true) {
                patch.abort = Some(true);
            }
            task.checkpoint = None;
        }
        let patch_field_count = [
            patch.status.is_some(),
            patch.checkpoint.is_some(),
            patch.abort.is_some(),
            patch.outcome.is_some(),
            patch.owns.is_some(),
        ]
        .iter()
        .filter(|field| **field)
        .count();
        if patch_field_count == 0 {
            return Ok(());
        }
        if task.status == TaskStatus::Terminal {
            self.with_doc(
                DocRef::Sticky {
                    conversation_id: task.conversation_id,
                },
                |state| {
                    if let Some(tasks) = state.get_mut("tasks").and_then(Value::as_object_mut) {
                        tasks.shift_remove(&task.id.to_string());
                    }
                    Ok(())
                },
            )?;
        }
        self.writes.push(Write::TaskPatch {
            patch: patch.clone(),
        });
        self.changes.tasks.push(task.clone());
        if task.status == TaskStatus::Terminal
            && prev.as_ref().map(|prev| prev.status) != Some(TaskStatus::Terminal)
        {
            if task.kind == "pi.collapse" {
                match &task.outcome {
                    Some(outcome) if outcome.status == "completed" => {
                        let summary = outcome
                            .result
                            .as_ref()
                            .and_then(|result| result.get("summary"))
                            .and_then(Value::as_i64)
                            .unwrap_or_default();
                        self.changes.events.push((
                            task.conversation_id,
                            ViewEvent::CompactionFinished {
                                task_id: task.id,
                                summary,
                            },
                        ));
                    }
                    Some(outcome) if outcome.status == "failed" => {
                        let (reason, detail) = outcome
                            .failure
                            .as_ref()
                            .map(|failure| {
                                (
                                    failure
                                        .get("reason")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_owned(),
                                    failure
                                        .get("detail")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_owned(),
                                )
                            })
                            .unwrap_or_default();
                        self.changes.events.push((
                            task.conversation_id,
                            ViewEvent::CompactionFailed {
                                task_id: task.id,
                                reason,
                                detail,
                            },
                        ));
                    }
                    _ => {}
                }
            } else if self
                .session
                .kinds
                .read()
                .expect("kind registry")
                .get(&task.kind)
                .map(|kind: &Arc<dyn super::types::AnyKind>| kind.turn())
                != Some(true)
                && task.outcome.is_some()
            {
                self.changes.events.push((
                    task.conversation_id,
                    ViewEvent::TaskEnded {
                        task_id: task.id,
                        kind: task.kind.clone(),
                        outcome: task.outcome.as_ref().expect("checked above").status.clone(),
                    },
                ));
            }
        }
        // Overlay: later direct reads see the patch (`session.ts:816`).
        self.created_tasks.insert(task.id, task);
        self.wrote_tasks = true;
        Ok(())
    }

    /// Upstream `createTask(spec)` (`session.ts:820-838`, by-name overload).
    pub(crate) fn create_task(&mut self, spec: TaskSpec) -> anyhow::Result<Id> {
        self.assert_core("createTask by name")?;
        self.create_task_internal(spec)
    }

    /// Upstream `createTask(kind, input, opts)` (`session.ts:820-838`, the
    /// token overload: the kind must be the registered token).
    pub fn create_task_kind(
        &mut self,
        kind: &Arc<dyn super::types::AnyKind>,
        input: Value,
        opts: CreateTaskOptions,
    ) -> anyhow::Result<TaskRef> {
        let registered = self
            .session
            .kinds
            .read()
            .expect("kind registry")
            .get(kind.name())
            .cloned();
        if registered.as_ref().map(Arc::as_ptr) != Some(Arc::as_ptr(kind)) {
            return Err(forbidden(format!(
                "createTask: kind \"{}\" is not the registered token",
                kind.name()
            )));
        }
        let id = self.create_task_internal(TaskSpec {
            kind: kind.name().to_owned(),
            conversation_id: opts.conversation_id,
            input,
            after: opts.after,
            background: opts.background,
        })?;
        Ok(TaskRef {
            id,
            kind: kind.clone(),
        })
    }

    fn create_task_internal(&mut self, spec: TaskSpec) -> anyhow::Result<Id> {
        let kind = self
            .session
            .kinds
            .read()
            .expect("kind registry")
            .get(&spec.kind)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown task kind {}", spec.kind))?;
        let kind_is_turn = kind.turn();
        if is_core_kind(&spec.kind) && !self.core() {
            return Err(forbidden(format!("create core task {}", spec.kind)));
        }
        let conversation_id = match spec.conversation_id {
            Some(conversation_id) => Some(conversation_id),
            None => match &self.invoker {
                Invoker::Task {
                    conversation_id, ..
                } => Some(*conversation_id),
                _ => None,
            },
        };
        let Some(conversation_id) = conversation_id else {
            anyhow::bail!("createTask: conversationId required");
        };
        self.assert_scope(conversation_id, "createTask")?;
        if spec.kind == "pi.generation" && self.has_live_kind(conversation_id, &spec.kind)? {
            return Err(generation_in_progress(conversation_id));
        }
        if spec.kind == "pi.collapse" && self.has_live_kind(conversation_id, &spec.kind)? {
            return Err(collapse_in_progress(conversation_id));
        }
        let id = self.session.storage.mint_id();
        let mut task = Task {
            id,
            conversation_id,
            kind: spec.kind.clone(),
            input: plain(&spec.input),
            status: TaskStatus::Pending,
            checkpoint: None,
            abort: None,
            outcome: None,
            after: spec.after.clone(),
            owns: Vec::new(),
            background: None,
        };
        if spec.background {
            task.background = Some(true);
        }
        self.writes.push(Write::Task { task: task.clone() });
        self.changes.tasks.push(task.clone());
        if spec.kind == "pi.collapse" {
            let reason = spec
                .input
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let through = spec
                .input
                .get("through")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            self.changes.events.push((
                conversation_id,
                ViewEvent::CompactionStarted {
                    task_id: id,
                    reason,
                    through,
                },
            ));
        } else if !kind_is_turn {
            self.changes.events.push((
                conversation_id,
                ViewEvent::TaskStarted {
                    task_id: id,
                    kind: task.kind.clone(),
                    background: task.background,
                },
            ));
        }
        self.created_tasks.insert(id, task);
        self.wrote_tasks = true;
        Ok(id)
    }

    /// Upstream `hasLiveKind` (`session.ts:885-903`).
    fn has_live_kind(&self, conversation_id: Id, kind: &str) -> anyhow::Result<bool> {
        let state = self.session.state.lock().expect("session state");
        for task in state.live_tasks.values() {
            if task.conversation_id != conversation_id || task.kind != kind {
                continue;
            }
            let current = self.created_tasks.get(&task.id).unwrap_or(task);
            if current.status == TaskStatus::Terminal {
                continue;
            }
            if self.closing && matches!(&self.invoker, Invoker::Task { id, .. } if *id == task.id) {
                continue;
            }
            return Ok(true);
        }
        for task in self.created_tasks.values() {
            if task.conversation_id == conversation_id
                && task.kind == kind
                && task.status != TaskStatus::Terminal
                && !state.live_tasks.contains_key(&task.id)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Upstream `createConversation` (`session.ts:905-909`).
    pub fn create_conversation(&mut self, spec: &ConversationSpec) -> anyhow::Result<Id> {
        let owner = match &self.invoker {
            Invoker::Task { id, .. } => Some(*id),
            _ => None,
        };
        if let Some(ConversationParentSpec::Parent {
            conversation_id, ..
        }) = spec.parent
        {
            self.assert_scope(conversation_id, "createConversation: parent")?;
        }
        self.insert_conversation(spec, owner, false)
    }

    /// Upstream `createForkConversation` (`session.ts:911-914`).
    pub(crate) fn create_fork_conversation(
        &mut self,
        spec: &ConversationSpec,
    ) -> anyhow::Result<Id> {
        self.assert_core("createForkConversation")?;
        self.insert_conversation(spec, None, true)
    }

    /// Upstream `createOwnedConversation` (`session.ts:916-939`).
    pub(crate) async fn create_owned_conversation(
        &mut self,
        owner_task_id: Id,
        source_conversation_id: Id,
        spec: OwnedConversationSpec,
    ) -> anyhow::Result<Id> {
        self.assert_core("createOwnedConversation")?;
        let owner = {
            let state = self.session.state.lock().expect("session state");
            self.created_tasks
                .get(&owner_task_id)
                .cloned()
                .or_else(|| state.live_tasks.get(&owner_task_id).cloned())
        };
        match owner {
            Some(owner)
                if owner.status != TaskStatus::Terminal
                    && owner.conversation_id == source_conversation_id => {}
            _ => {
                return Err(forbidden(format!(
                    "task {owner_task_id} cannot create an owned conversation"
                )));
            }
        }
        let tip = if spec.inherit {
            self.newest_entry(source_conversation_id, None, false)
                .await?
        } else {
            None
        };
        let inherited = match &tip {
            Some(tip) => {
                self.rewindable_as_of(source_conversation_id, tip.id)
                    .await?
            }
            None => None,
        };
        let mut raw_overrides = spec.rewindable.clone().unwrap_or_default();
        raw_overrides.shift_remove("plugins");
        let defaults = self.session.defaults();
        let overrides = defaults.validate_seed("rewindable", &raw_overrides)?;
        let inherited_plugins = inherited
            .as_ref()
            .and_then(|state| state.get("plugins"))
            .cloned();
        let had_inherited = inherited.is_some();
        let rewindable = match inherited {
            None => overrides,
            Some(mut inherited) => {
                inherited.extend(overrides);
                if let Some(plugins) = inherited_plugins {
                    inherited.insert("plugins".to_owned(), plugins);
                }
                inherited
            }
        };
        let conversation_spec = ConversationSpec {
            parent: tip.map(|tip| ConversationParentSpec::Parent {
                conversation_id: source_conversation_id,
                at: ParentAt::Id(tip.id),
            }),
            rewindable: Some(rewindable),
            sticky: spec.sticky.clone(),
            sections: None,
        };
        self.insert_conversation(&conversation_spec, Some(owner_task_id), had_inherited)
    }

    /// Upstream `insertConversation` (`session.ts:941-969`).
    fn insert_conversation(
        &mut self,
        spec: &ConversationSpec,
        owner: Option<Id>,
        preserve_plugins: bool,
    ) -> anyhow::Result<Id> {
        let id = self.session.storage.mint_id();
        let parent = match spec.parent {
            None
            | Some(ConversationParentSpec::Parent {
                at: ParentAt::Start,
                ..
            }) => None,
            Some(ConversationParentSpec::Parent {
                conversation_id,
                at: ParentAt::Id(at),
            }) => Some(super::types::ConversationParent {
                conversation_id,
                at,
            }),
        };
        let mut conversation = Conversation {
            id,
            parent,
            owner,
            sections: None,
        };
        if let Some(sections) = &spec.sections {
            if !sections.is_empty() {
                conversation.sections = Some(sections.clone());
            }
        }
        self.writes.push(Write::Conversation {
            conversation: conversation.clone(),
        });
        self.changes.conversations.push(conversation.clone());
        self.created_conversations.insert(id, conversation);
        let defaults = self.session.defaults();
        let rewindable = defaults.fresh_rewindable(
            spec.rewindable.as_ref().unwrap_or(&JsonObject::new()),
            preserve_plugins,
        )?;
        self.seed_doc(
            DocRef::Rewindable {
                conversation_id: id,
            },
            Value::Object(rewindable),
        )?;
        let sticky = defaults.fresh_sticky(spec.sticky.as_ref().unwrap_or(&JsonObject::new()))?;
        self.seed_doc(
            DocRef::Sticky {
                conversation_id: id,
            },
            Value::Object(sticky),
        )?;
        if let Some(owner) = owner {
            let task = {
                let state = self.session.state.lock().expect("session state");
                self.created_tasks
                    .get(&owner)
                    .cloned()
                    .or_else(|| state.live_tasks.get(&owner).cloned())
            };
            if let Some(mut task) = task {
                task.owns.push(id);
                self.set_task_internal(task)?;
            }
        }
        Ok(id)
    }

    /// Upstream `seedDoc` (`session.ts:970-978`): write the base batch for a
    /// new document and track it.
    fn seed_doc(&mut self, reference: DocRef, value: Value) -> anyhow::Result<()> {
        let mut tracker = track(value);
        tracker.rebase();
        let base = tracker.flush();
        self.writes.push(Write::Doc {
            r#ref: reference,
            ops: base.clone(),
        });
        self.changes.docs.push((reference, base));
        self.touched
            .insert(doc_key(&reference), TouchedDoc { reference, tracker });
        Ok(())
    }

    /// Upstream `markTask` (`session.ts:980-985`).
    pub(crate) fn mark_task(&mut self, id: Id) -> anyhow::Result<()> {
        self.assert_core("markTask")?;
        let task = {
            let state = self.session.state.lock().expect("session state");
            self.created_tasks
                .get(&id)
                .cloned()
                .or_else(|| state.live_tasks.get(&id).cloned())
        };
        let Some(mut task) = task else {
            anyhow::bail!("task {id} not live");
        };
        if task.abort != Some(true) {
            task.abort = Some(true);
            self.set_task_internal(task)?;
        }
        Ok(())
    }

    // --- admission ------------------------------------------------------------

    /// Prospective busy (`session.ts:989-1003`): live turn tasks, minus this
    /// transaction's terminals and the closing task, plus this transaction's
    /// new turn tasks.
    pub fn busy(&self, conversation_id: Id) -> anyhow::Result<bool> {
        let state = self.session.state.lock().expect("session state");
        let is_turn = |task: &Task| -> bool {
            self.session
                .kinds
                .read()
                .expect("kind registry")
                .get(&task.kind)
                .map(|kind| kind.turn())
                .unwrap_or(false)
                && task.background != Some(true)
        };
        for task in state.live_tasks.values() {
            if task.conversation_id != conversation_id || !is_turn(task) {
                continue;
            }
            if let Some(overlay) = self.created_tasks.get(&task.id) {
                if overlay.status == TaskStatus::Terminal {
                    continue;
                }
            }
            if self.closing && matches!(&self.invoker, Invoker::Task { id, .. } if *id == task.id) {
                continue;
            }
            return Ok(true);
        }
        for task in self.created_tasks.values() {
            if task.conversation_id == conversation_id
                && is_turn(task)
                && task.status != TaskStatus::Terminal
                && !state.live_tasks.contains_key(&task.id)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Upstream `putInput` (`session.ts:1005-1011`).
    fn put_input(&mut self, input: Input) {
        let input = plain_record(&input);
        self.writes.push(Write::Input {
            input: input.clone(),
        });
        self.changes.inputs.push(input.clone());
        self.inputs_by_id.insert(input.id, input.clone());
        if let Some(request_id) = &input.request_id {
            self.inputs_by_request
                .insert(format!("{}:{}", input.conversation_id, request_id), input);
        }
    }

    /// Upstream `send` (`session.ts:1013-1059`).
    pub async fn send(&mut self, conversation_id: Id, input: SendInput) -> anyhow::Result<Id> {
        self.assert_core("send")?;
        if let Some(request_id) = &input.request_id {
            // Before any write (`session.ts:1016`).
            let existing = self.input_by_request(conversation_id, request_id).await?;
            if let Some(existing) = existing {
                return Ok(existing.id);
            }
        }
        if self.busy(conversation_id)? {
            let mode = input
                .when_busy
                .clone()
                .unwrap_or_else(|| "followUp".to_owned());
            if mode == "reject" {
                return Err(conversation_busy(conversation_id));
            }
            let id = self.session.storage.mint_id();
            let mode_for_inbox = mode.clone();
            let stored_input = input.content.to_value();
            self.with_doc(DocRef::Sticky { conversation_id }, move |state| {
                let inbox = ensure_key_array(state, "inbox");
                inbox.push(Value::Object(JsonObject::from_iter([
                    ("id".to_owned(), number(id)),
                    ("mode".to_owned(), Value::String(mode_for_inbox)),
                    ("input".to_owned(), stored_input),
                ])));
                Ok(())
            })?;
            self.put_input(Input {
                id,
                conversation_id,
                request_id: input.request_id.clone(),
                status: "queued".to_owned(),
                entry: None,
                answer: None,
                reason: None,
                detail: None,
            });
            self.changes
                .events
                .push((conversation_id, ViewEvent::InputQueued { input: id, mode }));
            return Ok(id);
        }
        // Idle: read the head before the first append, place older queued
        // items, then this one (`session.ts:1037-1039`).
        let head = self.newest_entry(conversation_id, None, true).await?;
        let head_boundary = head.as_ref().map(|entry| entry.id);
        let boundary = self
            .boundary(conversation_id, "final", head_boundary)
            .await?;
        let id = self.session.storage.mint_id();
        let event_start = self.changes.events.len();
        let entry = self.append_entry_internal(
            conversation_id,
            NewEntry {
                kind: "pi.user".to_owned(),
                model: Some(vec![serde_json::json!({
                    "role": "user",
                    "content": input.content.to_value(),
                    "timestamp": self.session.now(),
                })]),
                data: None,
                head: None,
                edits: None,
            },
        )?;
        let entry_events: Vec<(Id, ViewEvent)> = self.changes.events.drain(event_start..).collect();
        self.put_input(Input {
            id,
            conversation_id,
            request_id: input.request_id.clone(),
            status: "placed".to_owned(),
            entry: Some(entry),
            answer: None,
            reason: None,
            detail: None,
        });
        self.changes
            .events
            .push((conversation_id, ViewEvent::InputPlaced { input: id, entry }));
        self.changes.events.extend(entry_events);
        let mut inputs = boundary.triggers;
        inputs.push(id);
        self.create_task_internal(TaskSpec {
            kind: "pi.generation".to_owned(),
            conversation_id: Some(conversation_id),
            input: serde_json::json!({ "inputs": inputs }),
            after: Vec::new(),
            background: false,
        })?;
        self.changes.events.push((
            conversation_id,
            ViewEvent::TurnStarted {
                inputs: inputs.clone(),
            },
        ));
        Ok(id)
    }

    /// Upstream `setInput` (`session.ts:1061-1065`).
    async fn set_input(&mut self, id: Id, patch: InputPatch) -> anyhow::Result<()> {
        let Some(mut current) = self.input(id).await? else {
            anyhow::bail!("input {id} not found");
        };
        if let Some(status) = patch.status {
            current.status = status;
        }
        if let Some(entry) = patch.entry {
            current.entry = Some(entry);
        }
        if let Some(answer) = patch.answer {
            current.answer = Some(answer);
        }
        if patch.reason_set {
            current.reason = patch.reason;
        }
        if patch.detail_set {
            current.detail = patch.detail;
        }
        self.put_input(current);
        Ok(())
    }

    /// Upstream `withdrawInput` (`session.ts:1067-1077`).
    pub async fn withdraw_input(&mut self, id: Id) -> anyhow::Result<&'static str> {
        self.assert_core("withdrawInput")?;
        let Some(input) = self.input(id).await? else {
            return Ok("not_found");
        };
        if input.status != "queued" {
            return Ok("already_placed");
        }
        self.preload(
            vec![DocRef::Sticky {
                conversation_id: input.conversation_id,
            }],
            self.ctx.clone(),
        )
        .await?;
        self.with_doc(
            DocRef::Sticky {
                conversation_id: input.conversation_id,
            },
            |state| {
                if let Some(inbox) = state.get_mut("inbox").and_then(Value::as_array_mut) {
                    inbox.retain(|queued| queued.get("id").and_then(Value::as_i64) != Some(id));
                }
                Ok(())
            },
        )?;
        self.set_input(
            id,
            InputPatch {
                status: Some("unanswered".to_owned()),
                reason: Some("aborted".to_owned()),
                reason_set: true,
                ..InputPatch::default()
            },
        )
        .await?;
        self.changes
            .events
            .push((input.conversation_id, ViewEvent::InputAborted { input: id }));
        Ok("aborted")
    }

    /// Upstream `resolveInputs` (`session.ts:1078-1086`).
    pub async fn resolve_inputs(
        &mut self,
        ids: &[Id],
        resolution: &Resolution,
    ) -> anyhow::Result<()> {
        self.assert_core("resolveInputs")?;
        for id in ids {
            self.set_input(
                *id,
                match resolution {
                    Resolution::Done { answer } => InputPatch {
                        status: Some("done".to_owned()),
                        answer: Some(*answer),
                        ..InputPatch::default()
                    },
                    Resolution::Unanswered { reason, detail } => InputPatch {
                        status: Some("unanswered".to_owned()),
                        reason: Some(reason.clone()),
                        reason_set: true,
                        detail: detail.clone(),
                        detail_set: true,
                        ..InputPatch::default()
                    },
                },
            )
            .await?;
        }
        Ok(())
    }

    /// Boundary placement (`session.ts:1088-1152`, pico §9.5): no storage
    /// scan; `head_boundary` is the newest head as the caller knows it, and
    /// it advances locally as same-batch self-heads are placed.
    pub async fn boundary(
        &mut self,
        conversation_id: Id,
        at: &str,
        head_boundary: Option<Id>,
    ) -> anyhow::Result<BoundaryOutcome> {
        self.assert_core("boundary")?;
        let sticky = self.doc_state(DocRef::Sticky { conversation_id })?;
        let mut inbox: Vec<(Id, String, Option<Head>, Value)> =
            sticky
                .get("inbox")
                .and_then(Value::as_array)
                .map(|queued| {
                    queued
                        .iter()
                        .filter_map(|queued| {
                            let id = queued.get("id").and_then(Value::as_i64)?;
                            let mode = queued.get("mode").and_then(Value::as_str)?.to_owned();
                            let head = queued.get("entry").and_then(|entry| entry.get("head")).map(
                                |head| {
                                    serde_json::from_value::<Head>(head.clone())
                                        .unwrap_or(Head::Id(0))
                                },
                            );
                            let payload = queued
                                .get(if mode == "write" { "entry" } else { "input" })
                                .cloned()
                                .unwrap_or(Value::Null);
                            Some((id, mode, head, payload))
                        })
                        .collect()
                })
                .unwrap_or_default();
        inbox.sort_by_key(|(id, ..)| *id);
        let mut cut: Option<Id> = None;
        for (id, mode, head, _) in &inbox {
            if mode == "write" && *head == Some(Head::Self_) {
                cut = Some(*id);
            }
        }
        let stale: Vec<Id> = match cut {
            None => Vec::new(),
            Some(cut) => inbox
                .iter()
                .filter(|(id, mode, ..)| *mode != "write" && *id < cut)
                .map(|(id, ..)| *id)
                .collect(),
        };
        for id in &stale {
            self.set_input(
                *id,
                InputPatch {
                    status: Some("unanswered".to_owned()),
                    reason: Some("stale".to_owned()),
                    reason_set: true,
                    ..InputPatch::default()
                },
            )
            .await?;
            self.changes
                .events
                .push((conversation_id, ViewEvent::InputAborted { input: *id }));
        }
        let survivors: Vec<&(Id, String, Option<Head>, Value)> = inbox
            .iter()
            .filter(|(id, ..)| !stale.contains(id))
            .collect();
        let sticky_doc = self.doc_state(DocRef::Sticky { conversation_id })?;
        let steering_mode = sticky_doc
            .get("steeringMode")
            .and_then(Value::as_str)
            .unwrap_or("one-at-a-time")
            .to_owned();
        let follow_up_mode = sticky_doc
            .get("followUpMode")
            .and_then(Value::as_str)
            .unwrap_or("one-at-a-time")
            .to_owned();
        let mut selected: HashSet<Id> = stale.iter().copied().collect();
        for (id, mode, ..) in &survivors {
            if mode == "write" {
                selected.insert(*id);
            }
        }
        let pick = |mode: &str, policy: &str| -> Vec<Id> {
            let items: Vec<Id> = survivors
                .iter()
                .filter(|(_, item_mode, ..)| item_mode == mode)
                .map(|(id, ..)| *id)
                .collect();
            if policy == "all" {
                items
            } else {
                items.into_iter().take(1).collect()
            }
        };
        for id in pick("steer", &steering_mode) {
            selected.insert(id);
        }
        if at == "final" {
            for id in pick("followUp", &follow_up_mode) {
                selected.insert(id);
            }
        }
        let mut head = head_boundary;
        let mut triggers: Vec<Id> = Vec::new();
        for survivor in survivors.iter().copied() {
            let (id, mode, queued_head, payload) = (
                survivor.0,
                survivor.1.clone(),
                survivor.2,
                survivor.3.clone(),
            );
            if !selected.contains(&id) {
                continue;
            }
            if mode == "write" {
                let entry_value: NewEntry = serde_json::from_value(payload.clone())?;
                if let Some(Head::Id(head_id)) = queued_head {
                    if head.is_some_and(|current| head_id < current) {
                        self.set_input(
                            id,
                            InputPatch {
                                status: Some("unanswered".to_owned()),
                                reason: Some("stale".to_owned()),
                                reason_set: true,
                                ..InputPatch::default()
                            },
                        )
                        .await?;
                        self.changes
                            .events
                            .push((conversation_id, ViewEvent::InputAborted { input: id }));
                        continue;
                    }
                }
                let event_start = self.changes.events.len();
                let entry = self.append_entry_internal(conversation_id, entry_value)?;
                let entry_events: Vec<(Id, ViewEvent)> =
                    self.changes.events.drain(event_start..).collect();
                match queued_head {
                    Some(Head::Self_) => head = Some(entry),
                    Some(Head::Id(head_id)) => head = Some(head_id),
                    None => {}
                }
                self.set_input(
                    id,
                    InputPatch {
                        status: Some("done".to_owned()),
                        entry: Some(entry),
                        ..InputPatch::default()
                    },
                )
                .await?;
                self.changes
                    .events
                    .push((conversation_id, ViewEvent::InputPlaced { input: id, entry }));
                self.changes.events.extend(entry_events);
            } else {
                let event_start = self.changes.events.len();
                let entry = self.append_entry_internal(
                    conversation_id,
                    NewEntry {
                        kind: "pi.user".to_owned(),
                        model: Some(vec![serde_json::json!({
                            "role": "user",
                            "content": payload,
                            "timestamp": self.session.now(),
                        })]),
                        data: None,
                        head: None,
                        edits: None,
                    },
                )?;
                let entry_events: Vec<(Id, ViewEvent)> =
                    self.changes.events.drain(event_start..).collect();
                self.set_input(
                    id,
                    InputPatch {
                        status: Some("placed".to_owned()),
                        entry: Some(entry),
                        ..InputPatch::default()
                    },
                )
                .await?;
                self.changes
                    .events
                    .push((conversation_id, ViewEvent::InputPlaced { input: id, entry }));
                self.changes.events.extend(entry_events);
                triggers.push(id);
            }
        }
        self.with_doc(DocRef::Sticky { conversation_id }, |state| {
            if let Some(stored) = state.get_mut("inbox").and_then(Value::as_array_mut) {
                stored.retain(|queued| {
                    let id = queued.get("id").and_then(Value::as_i64);
                    !id.is_some_and(|id| selected.contains(&id))
                });
            }
            Ok(())
        })?;
        Ok(BoundaryOutcome {
            triggers,
            terminated: cut.is_some(),
        })
    }

    // --- finish -------------------------------------------------------------

    /// Upstream `finish` (`session.ts:1156-1169`).
    pub(crate) fn finish(&mut self) -> anyhow::Result<Vec<Write>> {
        if let Some(poisoned) = &self.poisoned {
            // Upstream rethrows the poison itself (`session.ts:1157-1159`).
            return Err(anyhow::Error::new(poisoned.clone()));
        }
        for (conversation_id, keys) in &self.changed_config {
            self.changes.events.push((
                *conversation_id,
                ViewEvent::ConfigChanged { keys: keys.clone() },
            ));
        }
        let mut flushed: Vec<(String, DocRef, Vec<Op>)> = Vec::new();
        for (key, touched) in self.touched.iter_mut() {
            let ops = touched.tracker.flush();
            if ops.is_empty() {
                continue;
            }
            flushed.push((key.clone(), touched.reference, ops));
        }
        for (key, reference, ops) in &flushed {
            self.writes.push(Write::Doc {
                r#ref: *reference,
                ops: ops.clone(),
            });
            self.changes.docs.push((*reference, ops.clone()));
            let mut state = self.session.state.lock().expect("session state");
            state.docs.note_ops(key, ops);
        }
        Ok(self.writes.clone())
    }

    /// Upstream `revoke` (`session.ts:1170-1173`): every view handed out by
    /// this transaction fails from now on.
    pub fn revoke(&mut self) {
        self.membrane.revoke();
    }

    /// The transaction's invoker (upstream `readonly invoker`).
    pub fn invoker(&self) -> &Invoker {
        &self.invoker
    }

    /// The registered kind registry (upstream `readonly kinds`).
    pub fn kind_registry(&self) -> crate::agent_core::harness::pico3::session::KindRegistry {
        self.session.kinds().clone()
    }

    /// The session behind this transaction (the kinds read the clock
    /// through it). Not a public escape hatch from an ordinary task Tx.
    pub(crate) fn session(&self) -> &Arc<Session> {
        &self.session
    }
}

/// A mutable-field patch for [`Tx::set_input`] (upstream `Partial<Omit<Input,
/// "id" | "conversationId">>`, `session.ts:1061`).
#[derive(Debug, Default)]
struct InputPatch {
    status: Option<String>,
    entry: Option<Id>,
    answer: Option<Id>,
    reason: Option<String>,
    reason_set: bool,
    detail: Option<String>,
    detail_set: bool,
}

fn ensure_object(state: &mut Value) -> &mut JsonObject {
    if !state.is_object() {
        *state = Value::Object(JsonObject::new());
    }
    state.as_object_mut().expect("ensured object")
}

fn ensure_key_array<'v>(state: &'v mut Value, key: &str) -> &'v mut Vec<Value> {
    let object = ensure_object(state);
    object
        .entry(key.to_owned())
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .expect("ensured array")
}

fn ensure_object_entry<'v>(object: &'v mut JsonObject, key: &str) -> &'v mut JsonObject {
    object
        .entry(key.to_owned())
        .or_insert_with(|| Value::Object(JsonObject::new()))
        .as_object_mut()
        .expect("ensured object")
}

fn doc_name(reference: &DocRef) -> &'static str {
    match reference {
        DocRef::Session => "session",
        DocRef::Rewindable { .. } => "rewindable",
        DocRef::Sticky { .. } => "sticky",
    }
}

impl DocRef {
    /// Build a doc ref from a route document name (`"rewindable" | "sticky" |
    /// "session"`).
    pub fn from_name(name: &str, conversation_id: Id) -> DocRef {
        match name {
            "rewindable" => DocRef::Rewindable { conversation_id },
            "sticky" => DocRef::Sticky { conversation_id },
            _ => DocRef::Session,
        }
    }
}

/// The namespace slice view handed out by [`Tx::plugins`]
/// (`session.ts:565-591`): defaults materialize lazily on first access, and
/// writes land in the declaring document's slice.
pub struct PluginsView<'tx> {
    tx: &'tx mut Tx,
    namespace_id: String,
    routes: Vec<(String, String)>,
    defaults: super::types::NamespaceDefaultsValue,
    conversation_id: Id,
}

impl PluginsView<'_> {
    /// The declaring document of one key (upstream: the accessor pair built
    /// per route entry, `session.ts:572-586`).
    fn route_doc(&self, key: &str) -> anyhow::Result<String> {
        self.routes
            .iter()
            .find(|(route_key, _)| route_key == key)
            .map(|(_, doc)| doc.clone())
            .ok_or_else(|| anyhow::anyhow!("namespace key \"{key}\" is not declared"))
    }

    /// View construction (upstream `plugins()`, `session.ts:571-586`): for
    /// EVERY route entry, create the namespace slice in that route's
    /// document and materialize that key's declared default there.
    fn materialize_all(&mut self) -> anyhow::Result<()> {
        let routes = self.routes.clone();
        let namespace_id = self.namespace_id.clone();
        for (route_key, doc) in &routes {
            let reference = DocRef::from_name(doc, self.conversation_id);
            let default = match doc.as_str() {
                "rewindable" => self.defaults.rewindable.get(route_key).cloned(),
                "sticky" => self.defaults.sticky.get(route_key).cloned(),
                _ => self.defaults.session.get(route_key).cloned(),
            };
            let namespace_id = namespace_id.clone();
            let route_key = route_key.clone();
            self.tx.with_doc(reference, move |state| {
                let state = ensure_object(state);
                let plugins = ensure_object_entry(state, "plugins");
                let slice = ensure_object_entry(plugins, &namespace_id);
                if !slice.contains_key(&route_key) {
                    if let Some(default) = default {
                        slice.insert(route_key.clone(), plain(&default));
                    }
                }
                Ok(())
            })?;
        }
        Ok(())
    }

    /// Read one key (upstream `get`, `session.ts:581`); `None` when absent.
    pub fn get(&mut self, key: &str) -> anyhow::Result<Option<Value>> {
        self.materialize_all()?;
        let doc = self.route_doc(key)?;
        let reference = DocRef::from_name(&doc, self.conversation_id);
        let state = self.tx.doc_state(reference)?;
        Ok(state
            .get("plugins")
            .and_then(|plugins| plugins.get(&self.namespace_id))
            .and_then(|slice| slice.get(key))
            .cloned())
    }

    /// The merged slice across routed documents, as a plain copy (used by
    /// tests and `describe` projections).
    pub fn read(&mut self) -> anyhow::Result<JsonObject> {
        self.materialize_all()?;
        let route_keys: Vec<String> = self.routes.iter().map(|(key, _)| key.clone()).collect();
        let mut out = JsonObject::new();
        for key in &route_keys {
            let doc = self.route_doc(key)?;
            let reference = DocRef::from_name(&doc, self.conversation_id);
            let state = self.tx.doc_state(reference)?;
            if let Some(value) = state
                .get("plugins")
                .and_then(|plugins| plugins.get(&self.namespace_id))
                .and_then(|slice| slice.get(key))
            {
                out.insert(key.clone(), value.clone());
            }
        }
        Ok(out)
    }

    /// Write one key to its declaring document's slice (upstream `set`,
    /// `session.ts:582-584`).
    pub fn set(&mut self, key: &str, value: Value) -> anyhow::Result<()> {
        self.materialize_all()?;
        let doc = self.route_doc(key)?;
        let reference = DocRef::from_name(&doc, self.conversation_id);
        let namespace_id = self.namespace_id.clone();
        let key = key.to_owned();
        self.tx.with_doc(reference, move |state| {
            let state = ensure_object(state);
            let plugins = ensure_object_entry(state, "plugins");
            let slice = ensure_object_entry(plugins, &namespace_id);
            slice.insert(key, value);
            Ok(())
        })
    }
}

#[cfg(test)]
mod json_order_tests {
    use super::*;

    #[test]
    fn json_order_rewindable_defaults_preserve_override_rest() {
        let defaults = Defaults::default();
        let over = serde_json::json!({"plugins":{"p":1},"z":2,"a":3,"b":4});
        let before = over.to_string();
        let next = defaults
            .fresh_rewindable(over.as_object().unwrap(), true)
            .unwrap();
        assert_eq!(
            Value::Object(next).to_string(),
            r#"{"plugins":{"p":1},"z":2,"a":3,"b":4}"#
        );
        assert_eq!(over.to_string(), before);
    }

    #[test]
    fn json_order_sticky_defaults_strip_reserved_keys_without_reordering() {
        let mut defaults = Defaults::default();
        for key in ["z", "a", "b"] {
            defaults.route.insert(key.to_owned(), "sticky".to_owned());
        }
        let over = serde_json::json!({"inbox":[],"z":1,"plugins":{"x":1},"a":2,"turn":{},"tasks":{},"b":3});
        let before = over.to_string();
        let next = defaults.fresh_sticky(over.as_object().unwrap()).unwrap();
        assert_eq!(
            Value::Object(next).to_string(),
            r#"{"inbox":[],"turn":{"tools":[]},"tasks":{},"plugins":{},"z":1,"a":2,"b":3}"#
        );
        assert_eq!(over.to_string(), before);
    }
}
