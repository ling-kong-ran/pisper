//! Port of `packages/agent/src/harness/pico3/types.ts` (1038 lines): the
//! harness-v3 records, documents, storage vocabulary, kind metadata, view
//! shapes, invokers, and error taxonomy.
//!
//! # Disclosed substitutions
//!
//! - Upstream `Id`/`Seq` are JS numbers; the port uses `i64` ([`Id`],
//!   [`Seq`]). Every upstream id is a safe integer (`Number.isSafeInteger`
//!   guards in `memory.ts:63`), so the mapping is exact.
//! - `Message`/`StoredMessage` (`types.ts:73-81`) stay opaque JSON objects:
//!   storage never inspects them beyond `deriveContext`'s role/tool-result
//!   handling ([`crate::agent_core::harness::pico3::context`]), which works on
//!   the same JSON trees. The pi-ai message union is not a pico3 type.
//! - The type-level kind machinery (`KindTypes`, `TaskOf`/`InputOf`/`HooksOf`
//!   /`ConfigOf`/`SlotOf`, `DisjointConfig`, `ConfigOfKinds`, `Assert`/`IsJson`)
//!   is compile-time only and has no Rust equivalent; the runtime disjointness
//!   check it backs runs in [`session::Defaults::register`] (`session.ts:113-128`).
//!   `types.compile.ts`'s `@ts-expect-error` assertions (ordinary `TaskTx`
//!   does not expose core operations) are represented structurally: the port's
//!   transaction exposes host operations as `pub` methods and core operations
//!   as `pub(crate)` ([`session::Tx`]), so host code cannot name them.
//! - `HookResult`/`HookApi`/`HookBinding`/`HookRunner` (`types.ts:344-366`),
//!   `Step`/`Closure`/`AbortClosure`/`PhaseHandler` (`types.ts:369-404`),
//!   `Runtime`/`Models`/`RequestOptions` (`types.ts:808-854`), the tool
//!   surface (`types.ts:860-952`), and `Kind`'s `initial`/`phases`/`abort`
//!   execution surface land with the scheduler/harness (M3b Task 9/10). The
//!   port's erased kind ([`AnyKind`]) carries the metadata the storage
//!   engine and view read.
//! - `Stored<T>` (`types.ts:47`) is the `JsonRepresentation<T>` mapped type:
//!   over serde, the same guarantee is `T: Serialize` with
//!   [`crate::agent_core::JsonValue`] as the stored form.
//! - `QueuedInput` (`types.ts:209-211`) is a tagged enum; the queued write's
//!   `entry` field serializes exactly like upstream (`Stored<NewEntry>`).
//! - `ViewEvent` (`types.ts:650-698`) is hand-mapped to/from JSON
//!   ([`ViewEvent::to_value`]/[`ViewEvent::from_value`]) so every wire
//!   literal matches upstream, including the two `turn.ended` shapes and the
//!   dynamic `plugin.<namespace>.<name>` tag.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, RwLock};

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};

use crate::agent_core::chord_support::delta::{op_from_json, op_to_json, Op};
use crate::agent_core::chord_support::Context;

/// Upstream `Id`/`Seq` (`types.ts:49-50`): numeric record identifiers and
/// commit sequence numbers. See the module docs for the i64 note.
pub type Id = i64;
/// Upstream `Seq` (`types.ts:50`).
pub type Seq = i64;
/// Upstream `JsonObject` (`types.ts:37`): a strict JSON object.
pub type JsonObject = Map<String, Value>;
/// Upstream `RequestMessage = StoredMessage` (`types.ts:1038`).
pub type StoredMessage = Value;

/// Upstream `Checkpoint` (`types.ts:114`): a JSON object carrying `phase`.
pub type Checkpoint = JsonObject;

/// Read the `phase` field of a checkpoint (`types.ts:114`).
pub fn checkpoint_phase(checkpoint: &Checkpoint) -> Option<&str> {
    checkpoint.get("phase").and_then(Value::as_str)
}

/// Upstream `Conversation` (`types.ts:61-67`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conversation {
    pub id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ConversationParent>,
    /// The task that created it (`types.ts:64`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<Id>,
    /// §8.1 section seed; private, not in `ConversationView` (`types.ts:65-66`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sections: Option<Vec<SectionSeed>>,
}

/// Upstream `Conversation["parent"]` (`types.ts:63`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationParent {
    #[serde(rename = "conversationId")]
    pub conversation_id: Id,
    pub at: Id,
}

/// Upstream `SectionSeed` (`system.ts:48`): a set-with-checked-pair or a
/// remove marker for a new conversation's sections. Declared in `system.ts`
/// upstream; carried here because `Conversation` (this module's type) stores
/// it and the system module is Task 9 material.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SectionSeed {
    Set { key: String, value: Value },
    Remove { key: String },
}

/// Upstream `SystemMessage` (`types.ts:73-79`): system instructions riding
/// inside `messages` at their historical position. Shape-checked, never
/// interpreted, by this module's consumers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemMessageShape {
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools_added: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools_removed: Option<Value>,
    pub timestamp: i64,
}

/// Upstream `ContextEdit` (`types.ts:84-88`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextEdit {
    pub target: Id,
    /// `"omit" | "replace"`.
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<StoredMessage>>,
}

/// Upstream `Entry` (`types.ts:90-99`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: Id,
    #[serde(rename = "conversationId")]
    pub conversation_id: Id,
    pub kind: String,
    /// What the model sees; absent for display/bookkeeping entries
    /// (`types.ts:94`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<Vec<StoredMessage>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<JsonObject>,
    /// Context starts here; assigned by the kernel (`types.ts:96`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edits: Option<Vec<ContextEdit>>,
    #[serde(rename = "byTaskId", default, skip_serializing_if = "Option::is_none")]
    pub by_task_id: Option<Id>,
}

/// Upstream `NewEntry["head"]` (`types.ts:100`): an explicit head id or
/// `"self"` (this entry becomes the head).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Head {
    Id(Id),
    Self_,
}

impl Serialize for Head {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Head::Id(id) => serializer.serialize_i64(*id),
            Head::Self_ => serializer.serialize_str("self"),
        }
    }
}

impl<'de> Deserialize<'de> for Head {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        match &value {
            Value::String(text) if text == "self" => Ok(Head::Self_),
            Value::Number(number) => number
                .as_i64()
                .map(Head::Id)
                .ok_or_else(|| serde::de::Error::custom(format!("invalid head id: {number}"))),
            other => Err(serde::de::Error::custom(format!("invalid head: {other}"))),
        }
    }
}

/// Upstream `NewEntry` (`types.ts:100`): the caller-supplied entry shape;
/// the kernel assigns `id`, `conversationId`, `byTaskId`, and resolves
/// `head`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NewEntry {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<Vec<StoredMessage>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<JsonObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<Head>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edits: Option<Vec<ContextEdit>>,
}

impl NewEntry {
    /// Upstream object-literal convenience (`{ kind: "x", ... }`).
    pub fn new(kind: impl Into<String>) -> NewEntry {
        NewEntry {
            kind: kind.into(),
            ..NewEntry::default()
        }
    }
}

/// Upstream `EntryKind<E>` (`types.ts:103-106`): a typed witness for an
/// entry kind. `is` narrows; the port keeps the runtime check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryKind {
    pub kind: String,
}

impl EntryKind {
    /// Upstream `EntryKind#is` (`types.ts:105`).
    pub fn is(&self, entry: Option<&Entry>) -> bool {
        entry.is_some_and(|entry| entry.kind == self.kind)
    }
}

/// Upstream `defineEntry` (`types.ts:109-112`): kind names beginning with
/// `"pi."` are reserved.
pub fn define_entry(kind: impl Into<String>) -> anyhow::Result<EntryKind> {
    let kind = kind.into();
    if kind.starts_with("pi.") {
        anyhow::bail!("entry kind names beginning with \"pi.\" are reserved: {kind}");
    }
    Ok(EntryKind { kind })
}

/// Upstream `Completion` (`types.ts:116-118`): the terminal result shape a
/// kind's closures return. `result`/`failure` are strict JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Completion {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<Value>,
}

impl Completion {
    /// `{ status: "completed", result }` (`types.ts:117`).
    pub fn completed(result: Value) -> Completion {
        Completion {
            status: "completed".to_owned(),
            result: Some(result),
            failure: None,
        }
    }

    /// `{ status: "failed", failure }` (`types.ts:118`).
    pub fn failed(failure: Value) -> Completion {
        Completion {
            status: "failed".to_owned(),
            result: None,
            failure: Some(failure),
        }
    }
}

/// Upstream `Task["status"]` (`types.ts:131`). The wire literals are
/// lowercase (`"pending"` / `"running"` / `"terminal"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Pending,
    Running,
    Terminal,
}

/// Upstream `Outcome` (`types.ts:119-124`): how a task left the world. The
/// four payload variants are strict JSON; `faulted` is a contract breach, not
/// one of the kind's declared failures.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Outcome {
    /// Constructor for the `completed`/`failed`/`aborted` payload variants.
    pub fn with_status(status: &str, payload: Value) -> Outcome {
        let field = if status == "failed" {
            "failure"
        } else {
            "result"
        };
        let mut outcome = Outcome {
            status: status.to_owned(),
            result: None,
            failure: None,
            error: None,
        };
        if field == "failure" {
            outcome.failure = Some(payload);
        } else {
            outcome.result = Some(payload);
        }
        outcome
    }

    /// `{ status: "orphaned" }` (`types.ts:122`).
    pub fn orphaned() -> Outcome {
        Outcome {
            status: "orphaned".to_owned(),
            result: None,
            failure: None,
            error: None,
        }
    }

    /// `{ status: "faulted", error }` (`types.ts:123-124`).
    pub fn faulted(error: impl Into<String>) -> Outcome {
        Outcome {
            status: "faulted".to_owned(),
            result: None,
            failure: None,
            error: Some(error.into()),
        }
    }
}

/// Upstream `Task` (`types.ts:126-138`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: Id,
    #[serde(rename = "conversationId")]
    pub conversation_id: Id,
    pub kind: String,
    pub input: Value,
    pub status: TaskStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<Checkpoint>,
    /// The durable abort mark (`types.ts:133`); serialized only as `true`
    /// (the port stores `Some(true)` or `None`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abort: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
    pub after: Vec<Id>,
    /// Conversations this task created (`types.ts:136`).
    pub owns: Vec<Id>,
    /// Does not hold the conversation busy; survives conversation abort
    /// (`types.ts:137`); serialized only as `true` (the port stores
    /// `Some(true)` or `None`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<bool>,
}

impl Task {
    /// Whether the kind's turn flag makes this task hold the conversation
    /// busy (`types.ts:454-455` combined with `background`, `session.ts:991`).
    pub fn holds_busy(&self, kinds: &HashMap<String, Arc<dyn AnyKind>>) -> bool {
        kinds.get(&self.kind).is_some_and(|kind| kind.turn()) && self.background != Some(true)
    }
}

/// Upstream `TaskPatch` (`types.ts:139-141`): a partial task update.
/// `checkpoint: null` clears (`types.ts:300`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskPatch {
    pub id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TaskStatus>,
    /// `Some(None)` is the explicit `null` clear; `None` is absent.
    #[serde(
        default,
        deserialize_with = "deserialize_nullable_checkpoint",
        skip_serializing_if = "checkpoint_is_absent",
        serialize_with = "serialize_nullable_checkpoint"
    )]
    pub checkpoint: Option<Option<Checkpoint>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abort: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owns: Option<Vec<Id>>,
}

fn deserialize_nullable_checkpoint<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<Checkpoint>>, D::Error> {
    let value = Option::<Checkpoint>::deserialize(deserializer)?;
    Ok(Some(value))
}

fn serialize_nullable_checkpoint<S: serde::Serializer>(
    value: &Option<Option<Checkpoint>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match value {
        Some(inner) => inner.serialize(serializer),
        None => serializer.serialize_none(),
    }
}

fn checkpoint_is_absent(value: &Option<Option<Checkpoint>>) -> bool {
    value.is_none()
}

impl TaskPatch {
    /// Upstream `{ id }` minimal patch (`types.ts:139`).
    pub fn new(id: Id) -> TaskPatch {
        TaskPatch {
            id,
            status: None,
            checkpoint: None,
            abort: None,
            outcome: None,
            owns: None,
        }
    }

    /// Apply the patch's present fields onto `task`, honouring the explicit
    /// `null` checkpoint clear (`memory.ts:110-117`).
    pub fn apply_to(&self, task: &mut Task) {
        if let Some(status) = self.status {
            task.status = status;
        }
        match &self.checkpoint {
            Some(Some(checkpoint)) => task.checkpoint = Some(checkpoint.clone()),
            Some(None) => task.checkpoint = None,
            None => {}
        }
        if let Some(abort) = self.abort {
            task.abort = Some(abort);
        }
        if let Some(outcome) = &self.outcome {
            task.outcome = Some(outcome.clone());
        }
        if let Some(owns) = &self.owns {
            task.owns = owns.clone();
        }
    }
}

/// Upstream `Input` (`types.ts:143-152`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Input {
    pub id: Id,
    #[serde(rename = "conversationId")]
    pub conversation_id: Id,
    #[serde(rename = "requestId", default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Upstream `ModelRef` (`types.ts:158`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRef {
    pub provider: String,
    #[serde(rename = "modelId")]
    pub model_id: String,
}

/// Upstream `RetryPolicy` (`types.ts:159`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub enabled: bool,
    #[serde(rename = "maxRetries")]
    pub max_retries: i64,
    #[serde(rename = "baseDelayMs")]
    pub base_delay_ms: f64,
    #[serde(
        rename = "maxAgentDelayMs",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub max_agent_delay_ms: Option<f64>,
}

/// Upstream `QueuedInput` (`types.ts:209-211`): an untagged union — the
/// `mode` literal plus the payload key (`input` vs `entry`) distinguish the
/// variants on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum QueuedInput {
    /// `{ id, mode: "steer" | "followUp", input: Stored<UserInput> }`.
    Input {
        id: Id,
        mode: QueuedInputMode,
        input: Value,
    },
    /// `{ id, mode: "write", entry: Stored<NewEntry> }`.
    Write {
        id: Id,
        mode: QueuedInputMode,
        entry: Box<NewEntry>,
    },
}

/// Upstream `QueuedInput["mode"]` (`types.ts:210-211`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueuedInputMode {
    #[serde(rename = "steer")]
    Steer,
    #[serde(rename = "followUp")]
    FollowUp,
    #[serde(rename = "write")]
    Write,
}

impl QueuedInput {
    /// Upstream `q.id`.
    pub fn id(&self) -> Id {
        match self {
            QueuedInput::Input { id, .. } | QueuedInput::Write { id, .. } => *id,
        }
    }

    /// Upstream `q.mode`.
    pub fn mode(&self) -> QueuedInputMode {
        match self {
            QueuedInput::Input { mode, .. } | QueuedInput::Write { mode, .. } => *mode,
        }
    }
}

/// Upstream `DocRef` (`types.ts:213-216`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "doc", rename_all = "snake_case")]
pub enum DocRef {
    Session,
    Rewindable {
        #[serde(rename = "conversationId")]
        conversation_id: Id,
    },
    Sticky {
        #[serde(rename = "conversationId")]
        conversation_id: Id,
    },
}

impl DocRef {
    /// Upstream `docKey` (`session.ts:59`): the cache/log key for a document.
    pub fn key(&self) -> String {
        match self {
            DocRef::Session => "session".to_owned(),
            DocRef::Rewindable { conversation_id } => format!("rewindable:{conversation_id}"),
            DocRef::Sticky { conversation_id } => format!("sticky:{conversation_id}"),
        }
    }
}

/// Upstream `Write` (`types.ts:296-302`): the six write types one commit
/// carries. Tagged by `type` with the upstream literals. The wire mapping is
/// hand-rolled ([`write_to_json`]/[`write_from_json`]) because the chord
/// `Op` union (a `Doc` write's payload) has no derived serde impls by
/// design — its boundary is `op_from_json`/`op_to_json`.
#[derive(Debug, Clone, PartialEq)]
pub enum Write {
    Conversation {
        conversation: Conversation,
    },
    Entry {
        entry: Entry,
    },
    /// Create (`types.ts:299`).
    Task {
        task: Task,
    },
    /// `checkpoint: null` clears (`types.ts:300`).
    TaskPatch {
        patch: TaskPatch,
    },
    /// Create or replace whole (`types.ts:301`).
    Input {
        input: Input,
    },
    Doc {
        r#ref: DocRef,
        ops: Vec<Op>,
    },
}

impl Write {
    /// The upstream `type` literal.
    pub fn kind(&self) -> &'static str {
        match self {
            Write::Conversation { .. } => "conversation",
            Write::Entry { .. } => "entry",
            Write::Task { .. } => "task",
            Write::TaskPatch { .. } => "task.patch",
            Write::Input { .. } => "input",
            Write::Doc { .. } => "doc",
        }
    }
}

/// The persisted JSONL shape of a write (`jsonl.ts:24-28` records).
pub fn write_to_json(write: &Write) -> Value {
    use crate::agent_core::chord_support::delta::op_to_json;
    let mut object = JsonObject::new();
    object.insert("type".to_owned(), Value::String(write.kind().to_owned()));
    match write {
        Write::Conversation { conversation } => {
            object.insert(
                "conversation".to_owned(),
                serde_json::to_value(conversation).expect("conversation"),
            );
        }
        Write::Entry { entry } => {
            object.insert(
                "entry".to_owned(),
                serde_json::to_value(entry).expect("entry"),
            );
        }
        Write::Task { task } => {
            object.insert("task".to_owned(), serde_json::to_value(task).expect("task"));
        }
        Write::TaskPatch { patch } => {
            object.insert(
                "patch".to_owned(),
                serde_json::to_value(patch).expect("patch"),
            );
        }
        Write::Input { input } => {
            object.insert(
                "input".to_owned(),
                serde_json::to_value(input).expect("input"),
            );
        }
        Write::Doc { r#ref, ops } => {
            object.insert(
                "ref".to_owned(),
                serde_json::to_value(r#ref).expect("doc ref"),
            );
            object.insert(
                "ops".to_owned(),
                Value::Array(ops.iter().map(op_to_json).collect()),
            );
        }
    }
    Value::Object(object)
}

/// Parse the persisted JSONL shape of a write.
pub fn write_from_json(value: &Value) -> anyhow::Result<Write> {
    use crate::agent_core::chord_support::delta::op_from_json;
    let object = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("write is not an object"))?;
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("write lacks a type tag"))?;
    let take = |field: &str| -> anyhow::Result<Value> {
        object
            .get(field)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("write {kind} lacks {field}"))
    };
    let write = match kind {
        "conversation" => Write::Conversation {
            conversation: serde_json::from_value(take("conversation")?)?,
        },
        "entry" => Write::Entry {
            entry: serde_json::from_value(take("entry")?)?,
        },
        "task" => Write::Task {
            task: serde_json::from_value(take("task")?)?,
        },
        "task.patch" => Write::TaskPatch {
            patch: serde_json::from_value(take("patch")?)?,
        },
        "input" => Write::Input {
            input: serde_json::from_value(take("input")?)?,
        },
        "doc" => {
            let ops = object
                .get("ops")
                .and_then(Value::as_array)
                .map(|ops| ops.iter().map(op_from_json).collect::<Result<Vec<_>, _>>())
                .transpose()?
                .unwrap_or_default();
            Write::Doc {
                r#ref: serde_json::from_value(take("ref")?)?,
                ops,
            }
        }
        other => anyhow::bail!("unknown write type: {other}"),
    };
    Ok(write)
}

/// Upstream `EntryScan` (`types.ts:304-310`).
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryScan {
    #[serde(rename = "conversationId")]
    pub conversation_id: Id,
    pub kind: Option<String>,
    #[serde(rename = "withHead")]
    pub with_head: bool,
    /// Strictly less than (`types.ts:308`).
    pub before: Option<Id>,
    pub limit: usize,
}

/// Upstream `TaskScan` (`types.ts:311-315`).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskScan {
    #[serde(rename = "conversationId")]
    pub conversation_id: Option<Id>,
    pub status: Option<Vec<TaskStatus>>,
    pub kind: Option<String>,
}

/// Upstream `Storage` (`types.ts:317-336`): six write types, a handful of
/// reads, one owning Session per instance.
pub trait Storage: Send + Sync {
    /// Persist one batch atomically; returns the commit sequence
    /// (`types.ts:319`).
    fn commit<'a>(
        &'a self,
        writes: Vec<Write>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Seq>>;
    /// Mint the next unused id (`types.ts:320`); synchronous upstream.
    fn mint_id(&self) -> Id;
    fn conversation<'a>(
        &'a self,
        id: Id,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<Conversation>>>;
    fn conversations<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Conversation>>>;
    fn entries<'a>(
        &'a self,
        ids: &[Id],
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<HashMap<Id, Entry>>>;
    /// Newest-first, fork-aware (`types.ts:324-325`).
    fn scan_entries<'a>(
        &'a self,
        scan: &EntryScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Entry>>>;
    fn task<'a>(&'a self, id: Id, context: Context) -> BoxFuture<'a, anyhow::Result<Option<Task>>>;
    fn scan_tasks<'a>(
        &'a self,
        scan: &TaskScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Task>>>;
    fn input<'a>(
        &'a self,
        id: Id,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<Input>>>;
    fn input_by_request<'a>(
        &'a self,
        conversation_id: Id,
        request_id: &str,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<Input>>>;
    fn doc<'a>(
        &'a self,
        r#ref: &DocRef,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<JsonObject>>>;
    /// Rewindable doc as of the atomic commit containing entry `at`
    /// (`types.ts:331-332`).
    fn doc_as_of<'a>(
        &'a self,
        conversation_id: Id,
        at: Id,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<JsonObject>>>;
    /// Rewrite a doc log from its last base (`types.ts:333-334`).
    fn truncate<'a>(
        &'a self,
        r#ref: &DocRef,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    fn close<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<()>>;
}

/// Upstream `KindConfig` (`types.ts:409-412`): per-kind configuration
/// defaults, split by document. Values are JSON; the maps keep insertion
/// order via `serde_json`'s preserve-order feature when present, otherwise
/// sorted (disclosed: the port's disjointness/duplicate checks do not depend
/// on order).
///
/// Task-9 note (disclosed): upstream declares keys with an `undefined`
/// fallback (generation's `model: undefined as ModelRef | undefined`) —
/// routed, validated, but seeding no value. Rust's `Value` has no
/// `undefined`, and Task 8 ruled a declared `null` IS a value, so
/// declared-absent keys are listed in [`KindConfig::declared_absent`]
/// instead: they land in the route (and validate) without seeding a default.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KindConfig {
    pub rewindable: JsonObject,
    pub sticky: JsonObject,
    /// Keys routed without a default value (upstream `x: undefined`
    /// declarations).
    pub declared_absent: DeclaredAbsent,
}

/// The per-document declared-absent key lists ([`KindConfig`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeclaredAbsent {
    pub rewindable: Vec<String>,
    pub sticky: Vec<String>,
}

/// Upstream `AnyKind` (`types.ts:503-514`), erased to the metadata surface
/// the storage engine and view read. The `initial`/`phases`/`abort`
/// execution surface (upstream `Kind`, `types.ts:441-469`) lands with the
/// scheduler (M3b Task 9); implementors registered before that task provide
/// this metadata only.
pub trait AnyKind: Send + Sync {
    /// Upstream `name` (`types.ts:453`).
    fn name(&self) -> &str;
    /// A turn kind makes the conversation busy for admission (`types.ts:454-455`).
    fn turn(&self) -> bool {
        false
    }
    /// Declared config defaults (`types.ts:456`).
    fn config(&self) -> Option<&KindConfig> {
        None
    }
    /// Live slot (`sticky.tasks[id]`) initializer (`types.ts:457-458`).
    fn slot_init(&self, _input: &Value) -> JsonObject {
        JsonObject::new()
    }
    /// Rendering projection for a non-turn task (`types.ts:459-460`); raw
    /// checkpoints and slots are never published.
    fn describe(&self, task: &Task, slot: Option<&JsonObject>) -> anyhow::Result<Value> {
        let _ = (task, slot);
        Ok(default_describe(task))
    }
    /// Phases written by a handler immediately before an external effect
    /// (`types.ts:461`).
    fn inflight(&self) -> Vec<String> {
        Vec::new()
    }
}

/// The default describe projection (`view.ts:391-400`): the checkpoint phase,
/// or the status when there is no checkpoint yet.
pub fn default_describe(task: &Task) -> Value {
    let phase = task
        .checkpoint
        .as_ref()
        .and_then(checkpoint_phase)
        .map(|phase| Value::String(phase.to_owned()))
        .unwrap_or_else(|| {
            Value::String(
                match task.status {
                    TaskStatus::Pending => "pending",
                    TaskStatus::Running => "running",
                    TaskStatus::Terminal => "terminal",
                }
                .to_owned(),
            )
        });
    let mut object = JsonObject::new();
    object.insert("phase".to_owned(), phase);
    Value::Object(object)
}

/// Upstream `defineTask` (`types.ts:484-497`): author an ordinary kind;
/// names may not begin with `pi.`. The port validates the name and wraps the
/// metadata into a shared handle. The execution surface is Task 9 material
/// (see [`AnyKind`]).
pub fn define_task(definition: BasicKind) -> anyhow::Result<Arc<BasicKind>> {
    if definition.name.starts_with("pi.") {
        anyhow::bail!(
            "task kind names beginning with \"pi.\" are reserved: {}",
            definition.name
        );
    }
    Ok(Arc::new(definition))
}

/// The live-slot initializer type (upstream `slot?: (input) => S`,
/// `types.ts:457`).
pub type SlotInit = Arc<dyn Fn(&Value) -> JsonObject + Send + Sync>;
/// The rendering projection type (upstream `describe?: (task) => JsonValue`,
/// `types.ts:459-460`).
pub type DescribeFn =
    Arc<dyn Fn(&Task, Option<&JsonObject>) -> anyhow::Result<Value> + Send + Sync>;

/// A plain [`AnyKind`] implementation for ordinary kinds, tests, and the
/// built-in kinds' metadata (upstream: any `defineTask` result).
#[derive(Clone)]
pub struct BasicKind {
    name: String,
    turn: bool,
    config: Option<KindConfig>,
    slot: Option<SlotInit>,
    describe: Option<DescribeFn>,
    inflight: Vec<String>,
}

impl fmt::Debug for BasicKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BasicKind")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl BasicKind {
    /// Start a definition (`defineTask`'s argument, `types.ts:484`).
    pub fn new(name: impl Into<String>) -> BasicKind {
        BasicKind {
            name: name.into(),
            turn: false,
            config: None,
            slot: None,
            describe: None,
            inflight: Vec::new(),
        }
    }

    /// Upstream `turn?: true`.
    pub fn turn(mut self, turn: bool) -> BasicKind {
        self.turn = turn;
        self
    }

    /// Upstream `config?: Cfg`.
    pub fn config(mut self, config: KindConfig) -> BasicKind {
        self.config = Some(config);
        self
    }

    /// Upstream `slot?: (input) => S`.
    pub fn slot(
        mut self,
        slot: impl Fn(&Value) -> JsonObject + Send + Sync + 'static,
    ) -> BasicKind {
        self.slot = Some(Arc::new(slot));
        self
    }

    /// Upstream `describe?: (task) => JsonValue`.
    pub fn describe(
        mut self,
        describe: impl Fn(&Task, Option<&JsonObject>) -> anyhow::Result<Value> + Send + Sync + 'static,
    ) -> BasicKind {
        self.describe = Some(Arc::new(describe));
        self
    }

    /// Upstream `inflight?: readonly C["phase"][]`.
    pub fn inflight(mut self, phases: Vec<String>) -> BasicKind {
        self.inflight = phases;
        self
    }
}

impl AnyKind for BasicKind {
    fn name(&self) -> &str {
        &self.name
    }
    fn turn(&self) -> bool {
        self.turn
    }
    fn config(&self) -> Option<&KindConfig> {
        self.config.as_ref()
    }
    fn slot_init(&self, input: &Value) -> JsonObject {
        match &self.slot {
            Some(slot) => slot(input),
            None => JsonObject::new(),
        }
    }
    fn describe(&self, task: &Task, slot: Option<&JsonObject>) -> anyhow::Result<Value> {
        match &self.describe {
            Some(describe) => describe(task, slot),
            None => Ok(default_describe(task)),
        }
    }
    fn inflight(&self) -> Vec<String> {
        self.inflight.clone()
    }
}

/// Upstream `TaskSpec` (`types.ts:587-593`).
#[derive(Debug, Clone, PartialEq)]
pub struct TaskSpec {
    pub kind: String,
    pub conversation_id: Option<Id>,
    pub input: Value,
    pub after: Vec<Id>,
    pub background: bool,
}

/// Upstream `ConversationSpec` (`types.ts:599-604`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConversationSpec {
    pub parent: Option<ConversationParentSpec>,
    pub rewindable: Option<JsonObject>,
    pub sticky: Option<JsonObject>,
    pub sections: Option<Vec<SectionSeed>>,
}

/// Upstream `ConversationSpec["parent"]` (`types.ts:600`): `at` is an entry
/// id or `"start"` (no inheritance).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationParentSpec {
    Parent { conversation_id: Id, at: ParentAt },
}

/// Upstream `at: Id | "start"` (`types.ts:600`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParentAt {
    Id(Id),
    Start,
}

/// Upstream `OwnedConversationSpec` (`types.ts:605-609`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OwnedConversationSpec {
    pub inherit: bool,
    pub rewindable: Option<JsonObject>,
    pub sticky: Option<JsonObject>,
}

/// Upstream `UserInput` (`types.ts:610`): a string or a user-message content
/// payload; opaque JSON to this layer.
#[derive(Debug, Clone, PartialEq)]
pub enum UserInput {
    Text(String),
    Content(Value),
}

impl Default for UserInput {
    fn default() -> UserInput {
        UserInput::Text(String::new())
    }
}

impl UserInput {
    /// The stored JSON form (`Stored<UserInput>`).
    pub fn to_value(&self) -> Value {
        match self {
            UserInput::Text(text) => Value::String(text.clone()),
            UserInput::Content(content) => content.clone(),
        }
    }
}

/// Upstream `SendInput` (`types.ts:611-615`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SendInput {
    /// `UserInput::Text("")` — only the `Default` derive's shape; real sends
    /// always set `content`.
    pub content: UserInput,
    pub request_id: Option<String>,
    /// `"steer" | "followUp" | "reject"`; default `followUp` (`session.ts:1020`).
    pub when_busy: Option<String>,
}

/// Upstream `ContextView` (`types.ts:616`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ContextView {
    pub head: Option<Entry>,
    pub entries: Vec<Entry>,
    pub messages: Vec<StoredMessage>,
}

/// Upstream `GenerationStatus` (`types.ts:618-624`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum GenerationStatus {
    Waiting {
        on: String,
    },
    Preparing,
    Requesting {
        attempt: i64,
    },
    Streaming {
        attempt: i64,
    },
    Retrying {
        attempt: i64,
        #[serde(rename = "retryAt")]
        retry_at: i64,
        #[serde(rename = "lastError")]
        last_error: String,
    },
    Deferred {
        attempt: i64,
        #[serde(rename = "pollAt")]
        poll_at: i64,
    },
}

/// Upstream `TurnView` (`types.ts:626-631`): the current turn, shaped for
/// rendering. Tools are `ToolSlot`s without `memos`, kept as JSON objects.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TurnView {
    pub inputs: Vec<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<GenerationStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Value>,
    pub tools: Vec<Value>,
}

/// Upstream `ConversationView["compaction"]` (`types.ts:639-645`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactionView {
    #[serde(rename = "taskId")]
    pub task_id: Id,
    pub reason: String,
    pub stage: String,
    pub attempt: i64,
    #[serde(rename = "retryAt", default, skip_serializing_if = "Option::is_none")]
    pub retry_at: Option<i64>,
}

/// Upstream `ConversationView["tasks"]` values (`types.ts:646`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskViewSummary {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marked: Option<bool>,
    pub status: Value,
}

/// Upstream `ConversationView` (`types.ts:633-648`): the single flat
/// rendering document.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ConversationView {
    /// `Omit<Conversation, "sections">`, kept as JSON to preserve the exact
    /// record shape (`view.ts:153-155`).
    pub conversation: Value,
    pub entries: Vec<Entry>,
    pub config: JsonObject,
    pub inbox: Vec<QueuedInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<TurnView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<CompactionView>,
    pub tasks: HashMap<String, TaskViewSummary>,
    pub plugins: JsonObject,
}

/// Upstream `Envelope` (`types.ts:700-704`): one commit's view delta.
/// Serialized through [`Envelope::to_value`] (ops as delta tuples, events
/// through their wire mapping), since neither `Op` nor `ViewEvent` carries
/// derived serde impls.
#[derive(Debug, Clone, PartialEq)]
pub struct Envelope {
    pub revision: i64,
    pub ops: Vec<Op>,
    pub events: Vec<ViewEvent>,
}

impl Serialize for Envelope {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_value().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Envelope {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let object = value
            .as_object()
            .ok_or_else(|| serde::de::Error::custom("envelope is not an object"))?;
        let revision = object
            .get("revision")
            .and_then(Value::as_i64)
            .ok_or_else(|| serde::de::Error::custom("envelope lacks revision"))?;
        let ops = object
            .get("ops")
            .and_then(Value::as_array)
            .map(|ops| {
                ops.iter()
                    .map(op_from_json)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| serde::de::Error::custom(format!("{error}")))
            })
            .transpose()?
            .unwrap_or_default();
        let events = object
            .get("events")
            .and_then(Value::as_array)
            .map(|events| {
                events
                    .iter()
                    .map(ViewEvent::from_value)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(serde::de::Error::custom)
            })
            .transpose()?
            .unwrap_or_default();
        Ok(Envelope {
            revision,
            ops,
            events,
        })
    }
}

impl Envelope {
    /// Serialize ops through the delta tuple form; events through
    /// [`ViewEvent::to_value`].
    pub fn to_value(&self) -> Value {
        Value::Object(JsonObject::from_iter([
            (
                "revision".to_owned(),
                Value::Number(Number::from(self.revision)),
            ),
            (
                "ops".to_owned(),
                Value::Array(self.ops.iter().map(op_to_json).collect()),
            ),
            (
                "events".to_owned(),
                Value::Array(self.events.iter().map(ViewEvent::to_value).collect()),
            ),
        ]))
    }
}

/// Upstream `ViewEvent` (`types.ts:650-698`). Serialized with the upstream
/// `type` literals; `plugin.<namespace>.<name>` carries its namespace in the
/// tag and its payload in `data`.
#[derive(Debug, Clone, PartialEq)]
pub enum ViewEvent {
    EntryAdded {
        entry: Entry,
    },
    HeadMoved {
        entry: Entry,
    },
    TurnStarted {
        inputs: Vec<Id>,
    },
    TurnEndedDone {
        inputs: Vec<Id>,
        answer: Id,
    },
    TurnEndedUnanswered {
        inputs: Vec<Id>,
        reason: String,
        detail: Option<String>,
    },
    InputQueued {
        input: Id,
        mode: String,
    },
    InputPlaced {
        input: Id,
        entry: Id,
    },
    InputAborted {
        input: Id,
    },
    GenerationStarted {
        task_id: Id,
        attempt: i64,
    },
    GenerationRetrying {
        task_id: Id,
        attempt: i64,
        retry_at: i64,
        error: String,
    },
    GenerationDeferred {
        task_id: Id,
        poll_at: i64,
    },
    GenerationCompleted {
        task_id: Id,
        entry: Id,
        tool_calls: i64,
    },
    GenerationFailed {
        task_id: Id,
        reason: String,
        detail: String,
        entry: Option<Id>,
    },
    ToolWaiting {
        task_id: Id,
        call_id: String,
        on: String,
    },
    ToolStarted {
        task_id: Id,
        call_id: String,
        name: String,
    },
    ToolFinished {
        task_id: Id,
        call_id: String,
        entry: Id,
        is_error: bool,
        control: Option<Value>,
    },
    ToolAborted {
        task_id: Id,
        call_id: String,
        entry: Id,
    },
    CompactionStarted {
        task_id: Id,
        reason: String,
        through: Id,
    },
    CompactionRetrying {
        task_id: Id,
        attempt: i64,
        retry_at: i64,
        error: String,
    },
    CompactionFinished {
        task_id: Id,
        summary: Id,
    },
    CompactionFailed {
        task_id: Id,
        reason: String,
        detail: String,
    },
    TaskStarted {
        task_id: Id,
        kind: String,
        background: Option<bool>,
    },
    TaskEnded {
        task_id: Id,
        kind: String,
        outcome: String,
    },
    ConfigChanged {
        keys: Vec<String>,
    },
    Warning {
        source: String,
        message: String,
    },
    Plugin {
        namespace: String,
        name: String,
        data: Value,
    },
}

impl ViewEvent {
    /// The upstream `type` tag (`view.ts` consumers and `session.ts` emitters
    /// match on it).
    pub fn event_type(&self) -> String {
        match self {
            ViewEvent::EntryAdded { .. } => "entry.added".to_owned(),
            ViewEvent::HeadMoved { .. } => "head.moved".to_owned(),
            ViewEvent::TurnStarted { .. } => "turn.started".to_owned(),
            ViewEvent::TurnEndedDone { .. } | ViewEvent::TurnEndedUnanswered { .. } => {
                "turn.ended".to_owned()
            }
            ViewEvent::InputQueued { .. } => "input.queued".to_owned(),
            ViewEvent::InputPlaced { .. } => "input.placed".to_owned(),
            ViewEvent::InputAborted { .. } => "input.aborted".to_owned(),
            ViewEvent::GenerationStarted { .. } => "generation.started".to_owned(),
            ViewEvent::GenerationRetrying { .. } => "generation.retrying".to_owned(),
            ViewEvent::GenerationDeferred { .. } => "generation.deferred".to_owned(),
            ViewEvent::GenerationCompleted { .. } => "generation.completed".to_owned(),
            ViewEvent::GenerationFailed { .. } => "generation.failed".to_owned(),
            ViewEvent::ToolWaiting { .. } => "tool.waiting".to_owned(),
            ViewEvent::ToolStarted { .. } => "tool.started".to_owned(),
            ViewEvent::ToolFinished { .. } => "tool.finished".to_owned(),
            ViewEvent::ToolAborted { .. } => "tool.aborted".to_owned(),
            ViewEvent::CompactionStarted { .. } => "compaction.started".to_owned(),
            ViewEvent::CompactionRetrying { .. } => "compaction.retrying".to_owned(),
            ViewEvent::CompactionFinished { .. } => "compaction.finished".to_owned(),
            ViewEvent::CompactionFailed { .. } => "compaction.failed".to_owned(),
            ViewEvent::TaskStarted { .. } => "task.started".to_owned(),
            ViewEvent::TaskEnded { .. } => "task.ended".to_owned(),
            ViewEvent::ConfigChanged { .. } => "config.changed".to_owned(),
            ViewEvent::Warning { .. } => "warning".to_owned(),
            ViewEvent::Plugin {
                namespace, name, ..
            } => format!("plugin.{namespace}.{name}"),
        }
    }

    /// The exact JSON shape (`types.ts:650-698`).
    pub fn to_value(&self) -> Value {
        let mut object = JsonObject::new();
        object.insert("type".to_owned(), Value::String(self.event_type()));
        match self {
            ViewEvent::EntryAdded { entry } | ViewEvent::HeadMoved { entry } => {
                object.insert(
                    "entry".to_owned(),
                    serde_json::to_value(entry).expect("entry"),
                );
            }
            ViewEvent::TurnStarted { inputs } => {
                insert_ids(&mut object, "inputs", inputs);
            }
            ViewEvent::TurnEndedDone { inputs, answer } => {
                insert_ids(&mut object, "inputs", inputs);
                object.insert("status".to_owned(), Value::String("done".to_owned()));
                object.insert("answer".to_owned(), number(*answer));
            }
            ViewEvent::TurnEndedUnanswered {
                inputs,
                reason,
                detail,
            } => {
                insert_ids(&mut object, "inputs", inputs);
                object.insert("status".to_owned(), Value::String("unanswered".to_owned()));
                object.insert("reason".to_owned(), Value::String(reason.clone()));
                if let Some(detail) = detail {
                    object.insert("detail".to_owned(), Value::String(detail.clone()));
                }
            }
            ViewEvent::InputQueued { input, mode } => {
                object.insert("input".to_owned(), number(*input));
                object.insert("mode".to_owned(), Value::String(mode.clone()));
            }
            ViewEvent::InputPlaced { input, entry } => {
                object.insert("input".to_owned(), number(*input));
                object.insert("entry".to_owned(), number(*entry));
            }
            ViewEvent::InputAborted { input } => {
                object.insert("input".to_owned(), number(*input));
            }
            ViewEvent::GenerationStarted { task_id, attempt } => {
                object.insert("taskId".to_owned(), number(*task_id));
                object.insert("attempt".to_owned(), number(*attempt));
            }
            ViewEvent::GenerationRetrying {
                task_id,
                attempt,
                retry_at,
                error,
            } => {
                object.insert("taskId".to_owned(), number(*task_id));
                object.insert("attempt".to_owned(), number(*attempt));
                object.insert("retryAt".to_owned(), number(*retry_at));
                object.insert("error".to_owned(), Value::String(error.clone()));
            }
            ViewEvent::GenerationDeferred { task_id, poll_at } => {
                object.insert("taskId".to_owned(), number(*task_id));
                object.insert("pollAt".to_owned(), number(*poll_at));
            }
            ViewEvent::GenerationCompleted {
                task_id,
                entry,
                tool_calls,
            } => {
                object.insert("taskId".to_owned(), number(*task_id));
                object.insert("entry".to_owned(), number(*entry));
                object.insert("toolCalls".to_owned(), number(*tool_calls));
            }
            ViewEvent::GenerationFailed {
                task_id,
                reason,
                detail,
                entry,
            } => {
                object.insert("taskId".to_owned(), number(*task_id));
                object.insert("reason".to_owned(), Value::String(reason.clone()));
                object.insert("detail".to_owned(), Value::String(detail.clone()));
                if let Some(entry) = entry {
                    object.insert("entry".to_owned(), number(*entry));
                }
            }
            ViewEvent::ToolWaiting {
                task_id,
                call_id,
                on,
            } => {
                object.insert("taskId".to_owned(), number(*task_id));
                object.insert("callId".to_owned(), Value::String(call_id.clone()));
                object.insert("on".to_owned(), Value::String(on.clone()));
            }
            ViewEvent::ToolStarted {
                task_id,
                call_id,
                name,
            } => {
                object.insert("taskId".to_owned(), number(*task_id));
                object.insert("callId".to_owned(), Value::String(call_id.clone()));
                object.insert("name".to_owned(), Value::String(name.clone()));
            }
            ViewEvent::ToolFinished {
                task_id,
                call_id,
                entry,
                is_error,
                control,
            } => {
                object.insert("taskId".to_owned(), number(*task_id));
                object.insert("callId".to_owned(), Value::String(call_id.clone()));
                object.insert("entry".to_owned(), number(*entry));
                object.insert("isError".to_owned(), Value::Bool(*is_error));
                if let Some(control) = control {
                    object.insert("control".to_owned(), control.clone());
                }
            }
            ViewEvent::ToolAborted {
                task_id,
                call_id,
                entry,
            } => {
                object.insert("taskId".to_owned(), number(*task_id));
                object.insert("callId".to_owned(), Value::String(call_id.clone()));
                object.insert("entry".to_owned(), number(*entry));
            }
            ViewEvent::CompactionStarted {
                task_id,
                reason,
                through,
            } => {
                object.insert("taskId".to_owned(), number(*task_id));
                object.insert("reason".to_owned(), Value::String(reason.clone()));
                object.insert("through".to_owned(), number(*through));
            }
            ViewEvent::CompactionRetrying {
                task_id,
                attempt,
                retry_at,
                error,
            } => {
                object.insert("taskId".to_owned(), number(*task_id));
                object.insert("attempt".to_owned(), number(*attempt));
                object.insert("retryAt".to_owned(), number(*retry_at));
                object.insert("error".to_owned(), Value::String(error.clone()));
            }
            ViewEvent::CompactionFinished { task_id, summary } => {
                object.insert("taskId".to_owned(), number(*task_id));
                object.insert("summary".to_owned(), number(*summary));
            }
            ViewEvent::CompactionFailed {
                task_id,
                reason,
                detail,
            } => {
                object.insert("taskId".to_owned(), number(*task_id));
                object.insert("reason".to_owned(), Value::String(reason.clone()));
                object.insert("detail".to_owned(), Value::String(detail.clone()));
            }
            ViewEvent::TaskStarted {
                task_id,
                kind,
                background,
            } => {
                object.insert("taskId".to_owned(), number(*task_id));
                object.insert("kind".to_owned(), Value::String(kind.clone()));
                if *background == Some(true) {
                    object.insert("background".to_owned(), Value::Bool(true));
                }
            }
            ViewEvent::TaskEnded {
                task_id,
                kind,
                outcome,
            } => {
                object.insert("taskId".to_owned(), number(*task_id));
                object.insert("kind".to_owned(), Value::String(kind.clone()));
                object.insert("outcome".to_owned(), Value::String(outcome.clone()));
            }
            ViewEvent::ConfigChanged { keys } => {
                object.insert(
                    "keys".to_owned(),
                    Value::Array(keys.iter().map(|key| Value::String(key.clone())).collect()),
                );
            }
            ViewEvent::Warning { source, message } => {
                object.insert("source".to_owned(), Value::String(source.clone()));
                object.insert("message".to_owned(), Value::String(message.clone()));
            }
            ViewEvent::Plugin { data, .. } => {
                object.insert("data".to_owned(), data.clone());
            }
        }
        Value::Object(object)
    }

    /// Parse the upstream JSON shape; the `plugin.` tag prefix splits into
    /// namespace and name (`types.ts:698`).
    pub fn from_value(value: &Value) -> anyhow::Result<ViewEvent> {
        let object = value
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("view event is not an object"))?;
        let tag = object
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("view event lacks a type tag"))?;
        let field_id = |key: &str| -> anyhow::Result<Id> {
            object
                .get(key)
                .and_then(Value::as_i64)
                .ok_or_else(|| anyhow::anyhow!("view event {tag} lacks {key}"))
        };
        let field_string = |key: &str| -> anyhow::Result<String> {
            object
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| anyhow::anyhow!("view event {tag} lacks {key}"))
        };
        let field_i64 = |key: &str| -> anyhow::Result<i64> {
            object
                .get(key)
                .and_then(Value::as_i64)
                .ok_or_else(|| anyhow::anyhow!("view event {tag} lacks {key}"))
        };
        let ids = |key: &str| -> anyhow::Result<Vec<Id>> {
            Ok(object
                .get(key)
                .and_then(Value::as_array)
                .map(|array| array.iter().filter_map(Value::as_i64).collect())
                .unwrap_or_default())
        };
        let event = match tag {
            "entry.added" => ViewEvent::EntryAdded {
                entry: entry_field(object)?,
            },
            "head.moved" => ViewEvent::HeadMoved {
                entry: entry_field(object)?,
            },
            "turn.started" => ViewEvent::TurnStarted {
                inputs: ids("inputs")?,
            },
            "turn.ended" if object.get("status").and_then(Value::as_str) == Some("done") => {
                ViewEvent::TurnEndedDone {
                    inputs: ids("inputs")?,
                    answer: field_id("answer")?,
                }
            }
            "turn.ended" => ViewEvent::TurnEndedUnanswered {
                inputs: ids("inputs")?,
                reason: field_string("reason")?,
                detail: object
                    .get("detail")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            },
            "input.queued" => ViewEvent::InputQueued {
                input: field_id("input")?,
                mode: field_string("mode")?,
            },
            "input.placed" => ViewEvent::InputPlaced {
                input: field_id("input")?,
                entry: field_id("entry")?,
            },
            "input.aborted" => ViewEvent::InputAborted {
                input: field_id("input")?,
            },
            "generation.started" => ViewEvent::GenerationStarted {
                task_id: field_id("taskId")?,
                attempt: field_i64("attempt")?,
            },
            "generation.retrying" => ViewEvent::GenerationRetrying {
                task_id: field_id("taskId")?,
                attempt: field_i64("attempt")?,
                retry_at: field_i64("retryAt")?,
                error: field_string("error")?,
            },
            "generation.deferred" => ViewEvent::GenerationDeferred {
                task_id: field_id("taskId")?,
                poll_at: field_i64("pollAt")?,
            },
            "generation.completed" => ViewEvent::GenerationCompleted {
                task_id: field_id("taskId")?,
                entry: field_id("entry")?,
                tool_calls: field_i64("toolCalls")?,
            },
            "generation.failed" => ViewEvent::GenerationFailed {
                task_id: field_id("taskId")?,
                reason: field_string("reason")?,
                detail: field_string("detail")?,
                entry: object.get("entry").and_then(Value::as_i64),
            },
            "tool.waiting" => ViewEvent::ToolWaiting {
                task_id: field_id("taskId")?,
                call_id: field_string("callId")?,
                on: field_string("on")?,
            },
            "tool.started" => ViewEvent::ToolStarted {
                task_id: field_id("taskId")?,
                call_id: field_string("callId")?,
                name: field_string("name")?,
            },
            "tool.finished" => ViewEvent::ToolFinished {
                task_id: field_id("taskId")?,
                call_id: field_string("callId")?,
                entry: field_id("entry")?,
                is_error: object
                    .get("isError")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                control: object.get("control").cloned(),
            },
            "tool.aborted" => ViewEvent::ToolAborted {
                task_id: field_id("taskId")?,
                call_id: field_string("callId")?,
                entry: field_id("entry")?,
            },
            "compaction.started" => ViewEvent::CompactionStarted {
                task_id: field_id("taskId")?,
                reason: field_string("reason")?,
                through: field_id("through")?,
            },
            "compaction.retrying" => ViewEvent::CompactionRetrying {
                task_id: field_id("taskId")?,
                attempt: field_i64("attempt")?,
                retry_at: field_i64("retryAt")?,
                error: field_string("error")?,
            },
            "compaction.finished" => ViewEvent::CompactionFinished {
                task_id: field_id("taskId")?,
                summary: field_id("summary")?,
            },
            "compaction.failed" => ViewEvent::CompactionFailed {
                task_id: field_id("taskId")?,
                reason: field_string("reason")?,
                detail: field_string("detail")?,
            },
            "task.started" => ViewEvent::TaskStarted {
                task_id: field_id("taskId")?,
                kind: field_string("kind")?,
                background: match object.get("background") {
                    Some(Value::Bool(true)) => Some(true),
                    _ => None,
                },
            },
            "task.ended" => ViewEvent::TaskEnded {
                task_id: field_id("taskId")?,
                kind: field_string("kind")?,
                outcome: field_string("outcome")?,
            },
            "config.changed" => ViewEvent::ConfigChanged {
                keys: object
                    .get("keys")
                    .and_then(Value::as_array)
                    .map(|array| {
                        array
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
            },
            "warning" => ViewEvent::Warning {
                source: field_string("source")?,
                message: field_string("message")?,
            },
            plugin if plugin.starts_with("plugin.") => {
                let rest = &plugin["plugin.".len()..];
                let (namespace, name) = rest
                    .split_once('.')
                    .ok_or_else(|| anyhow::anyhow!("malformed plugin event tag: {plugin}"))?;
                ViewEvent::Plugin {
                    namespace: namespace.to_owned(),
                    name: name.to_owned(),
                    data: object.get("data").cloned().unwrap_or(Value::Null),
                }
            }
            other => anyhow::bail!("unknown view event type: {other}"),
        };
        Ok(event)
    }
}

fn entry_field(object: &JsonObject) -> anyhow::Result<Entry> {
    let entry = object
        .get("entry")
        .ok_or_else(|| anyhow::anyhow!("view event lacks entry"))?;
    Ok(serde_json::from_value(entry.clone())?)
}

fn insert_ids(object: &mut JsonObject, key: &str, ids: &[Id]) {
    object.insert(
        key.to_owned(),
        Value::Array(ids.iter().map(|id| number(*id)).collect()),
    );
}

/// A JSON number for an id/seq (`i64` upstream number).
pub fn number(value: i64) -> Value {
    Value::Number(Number::from(value))
}

/// Upstream `NamespaceRegistration` (`types.ts:233-238`): the internal erased
/// registration paired with the public namespace token. Token identity is a
/// generation counter (upstream: the namespace object's identity; stale
/// tokens are rejected, `session.ts:565-567`).
/// The namespace projection type (`types.ts:237`).
pub type ProjectFn = Arc<dyn Fn(&JsonObject) -> anyhow::Result<Value> + Send + Sync>;

#[derive(Clone)]
pub struct NamespaceRegistration {
    /// Identity generation; `Namespace.generation` must match.
    pub generation: u64,
    pub defaults: NamespaceDefaultsValue,
    /// Per-key route to the declaring document (`types.ts:236`).
    pub routes: Vec<(String, String)>,
    /// Rendering projection for `ConversationView.plugins` (`types.ts:237`,
    /// `view.ts:246-257`).
    pub project: Option<ProjectFn>,
}

/// Upstream `NamespaceRegistration["defaults"]` (`types.ts:235`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NamespaceDefaultsValue {
    pub rewindable: JsonObject,
    pub sticky: JsonObject,
    pub session: JsonObject,
}

/// Upstream `Namespace<T>` (`types.ts:225-230`): the current process
/// authority for one durable namespace string. `generation` is the token
/// identity; `unregister` is the harness's re-registration control (Task 9
/// surface — the registration map lives on the Session).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Namespace {
    pub id: String,
    pub generation: u64,
}

/// Upstream `memoOnce` (`types.ts:241-247`): first writer wins, including
/// when the stored winner is null. Operates on the `memos` slice of a tool
/// slot object.
pub fn memo_once(slot: &mut JsonObject, key: &str, candidate: Value) -> Value {
    if !slot.contains_key("memos") {
        slot.insert("memos".to_owned(), Value::Object(JsonObject::new()));
    }
    let Some(Value::Object(memos)) = slot.get_mut("memos") else {
        unreachable!("memo_once inserts the memos object above")
    };
    if let Some(existing) = memos.get(key) {
        return existing.clone();
    }
    memos.insert(key.to_owned(), candidate.clone());
    candidate
}

/// Upstream `ReadAfterWrite` (`types.ts:797-802`): thrown when a scan-shaped
/// read follows a same-batch write to its domain; poisons the transaction.
/// Carried as a typed error so callers can match on the type, not the
/// message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadAfterWrite {
    pub read: String,
    pub write: String,
}

impl ReadAfterWrite {
    pub fn new(read: impl Into<String>, write: impl Into<String>) -> Self {
        ReadAfterWrite {
            read: read.into(),
            write: write.into(),
        }
    }

    /// The upstream message (`types.ts:799`).
    pub fn message(&self) -> String {
        format!(
            "{} after {} in the same transaction: the answer would not include the buffered write",
            self.read, self.write
        )
    }

    /// The `name` upstream error classes carry; used by error-type matchers.
    pub fn name(&self) -> &'static str {
        "ReadAfterWrite"
    }
}

impl fmt::Display for ReadAfterWrite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message())
    }
}

impl std::error::Error for ReadAfterWrite {}

/// Downcast marker names for the upstream error classes
/// (`types.ts:994-1035`). The port's module surfaces return
/// `anyhow::Error`; these helpers attach/detect the upstream `name` so
/// oracle tests can match error kinds, not just messages.
pub const FORBIDDEN: &str = "Forbidden";
pub const CONVERSATION_BUSY: &str = "ConversationBusy";
pub const GENERATION_IN_PROGRESS: &str = "GenerationInProgress";
pub const COLLAPSE_IN_PROGRESS: &str = "CollapseInProgress";
pub const FAULTED: &str = "Faulted";
pub const CLOSED: &str = "Closed";
pub const TASK_CONTRACT_FAULT: &str = "TaskContractFault";
pub const NESTED_LINE_OPERATION: &str = "NestedLineOperation";

/// Attach an upstream error-class name to an error (the `name` property).
/// `Error::new` (not `Error::msg`) stores the typed error so `downcast_ref`
/// finds it.
pub fn named_error(name: &'static str, message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(NamedMessage {
        name,
        message: message.into(),
    })
}

/// A message carrying the upstream error-class name; `Display` prints only
/// the message, matching upstream `Error#message` semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedMessage {
    pub name: &'static str,
    pub message: String,
}

impl fmt::Display for NamedMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for NamedMessage {}

/// True when the error chain carries the upstream class name.
pub fn is_named(error: &anyhow::Error, name: &str) -> bool {
    error
        .chain()
        .filter_map(|cause| cause.downcast_ref::<NamedMessage>())
        .any(|named| named.name == name)
        || error
            .chain()
            .filter_map(|cause| cause.downcast_ref::<ReadAfterWrite>())
            .any(|_| name == "ReadAfterWrite")
}

/// Upstream `Forbidden` (`types.ts:994-999`).
pub fn forbidden(what: impl fmt::Display) -> anyhow::Error {
    named_error(FORBIDDEN, format!("forbidden: {what}"))
}

/// Upstream `ConversationBusy` (`types.ts:1000-1005`).
pub fn conversation_busy(id: Id) -> anyhow::Error {
    named_error(CONVERSATION_BUSY, format!("conversation {id} is busy"))
}

/// Upstream `GenerationInProgress` (`types.ts:1006-1011`).
pub fn generation_in_progress(id: Id) -> anyhow::Error {
    named_error(
        GENERATION_IN_PROGRESS,
        format!("conversation {id} already has a live generation"),
    )
}

/// Upstream `CollapseInProgress` (`types.ts:1012-1017`).
pub fn collapse_in_progress(id: Id) -> anyhow::Error {
    named_error(
        COLLAPSE_IN_PROGRESS,
        format!("conversation {id} already has a live collapse"),
    )
}

/// Upstream `Faulted` (`types.ts:1018-1023`).
pub fn faulted(cause: impl fmt::Display) -> anyhow::Error {
    named_error(FAULTED, format!("Session faulted: {cause}"))
}

/// Upstream `Closed` (`types.ts:1024-1029`).
pub fn closed() -> anyhow::Error {
    named_error(CLOSED, "Session is closed")
}

/// Upstream `TaskContractFault` (`types.ts:1030-1035`).
pub fn task_contract_fault(kind: &str, what: &str) -> anyhow::Error {
    named_error(
        TASK_CONTRACT_FAULT,
        format!("kind {kind} broke its contract: {what}"),
    )
}

/// Upstream `NestedLineOperation` (`session.ts:1198-1203`), declared next to
/// the other error classes for one error taxonomy.
pub fn nested_line_operation() -> anyhow::Error {
    named_error(NESTED_LINE_OPERATION, "nested line operation")
}

/// Upstream `CORE_KINDS` (`session.ts:66`): the fixed core kinds.
pub fn is_core_kind(name: &str) -> bool {
    matches!(
        name,
        "pi.generation" | "pi.tool" | "pi.post_tools" | "pi.collapse"
    )
}

/// Upstream `InvocationToken` (`types.ts:959-974`): an unforgeable
/// capability, one per invocation. `alive` flips when the scheduler revokes
/// the invocation.
#[derive(Debug)]
pub struct InvocationToken {
    alive: RwLock<bool>,
}

impl InvocationToken {
    pub fn new() -> Arc<InvocationToken> {
        Arc::new(InvocationToken {
            alive: RwLock::new(true),
        })
    }

    /// Upstream `get alive` (`types.ts:967-969`).
    pub fn alive(&self) -> bool {
        *self.alive.read().expect("token lock")
    }

    /// Upstream `revoke()` (`types.ts:971-973`).
    pub fn revoke(&self) {
        *self.alive.write().expect("token lock") = false;
    }
}

impl Default for InvocationToken {
    fn default() -> Self {
        InvocationToken {
            alive: RwLock::new(true),
        }
    }
}

/// Upstream `Invoker` (`types.ts:976-988`): who is driving a transaction.
#[derive(Clone)]
pub enum Invoker {
    /// `{ type: "host" }`.
    Host { conversation_id: Option<Id> },
    /// `{ type: "kernel" }` — harness internals with core authority.
    Kernel { conversation_id: Option<Id> },
    /// `{ type: "task", token, id, conversationId, kind, core, mode }`.
    Task {
        token: Arc<InvocationToken>,
        id: Id,
        conversation_id: Id,
        kind: Arc<dyn AnyKind>,
        core: bool,
        mode: InvocationMode,
    },
}

/// Upstream `Invoker["mode"]` (`types.ts:962-963, 987`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvocationMode {
    Run,
    Abort,
}

impl Invoker {
    /// The invoker's bound conversation, when it has one (`types.ts:612-617`
    /// reads this).
    pub fn conversation_id(&self) -> Option<Id> {
        match self {
            Invoker::Host { conversation_id } | Invoker::Kernel { conversation_id } => {
                *conversation_id
            }
            Invoker::Task {
                conversation_id, ..
            } => Some(*conversation_id),
        }
    }

    /// Upstream `invoker.type`.
    pub fn type_name(&self) -> &'static str {
        match self {
            Invoker::Host { .. } => "host",
            Invoker::Kernel { .. } => "kernel",
            Invoker::Task { .. } => "task",
        }
    }

    /// The task invoker's id, when the invoker is a task.
    pub fn task_id(&self) -> Option<Id> {
        match self {
            Invoker::Task { id, .. } => Some(*id),
            _ => None,
        }
    }

    /// The task invoker's kind handle, when the invoker is a task.
    pub fn task_kind(&self) -> Option<&Arc<dyn AnyKind>> {
        match self {
            Invoker::Task { kind, .. } => Some(kind),
            _ => None,
        }
    }

    /// Upstream `invoker.core`.
    pub fn is_core(&self) -> bool {
        matches!(self, Invoker::Kernel { .. }) || matches!(self, Invoker::Task { core: true, .. })
    }
}

impl fmt::Debug for Invoker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Invoker::Host { conversation_id } => f
                .debug_struct("Invoker::Host")
                .field("conversation_id", conversation_id)
                .finish(),
            Invoker::Kernel { conversation_id } => f
                .debug_struct("Invoker::Kernel")
                .field("conversation_id", conversation_id)
                .finish(),
            Invoker::Task {
                id,
                conversation_id,
                core,
                mode,
                ..
            } => f
                .debug_struct("Invoker::Task")
                .field("id", id)
                .field("conversation_id", conversation_id)
                .field("core", core)
                .field("mode", mode)
                .finish(),
        }
    }
}
/// Upstream `TxReads`' snapshot summary: docs the tx touched, used by
/// [`crate::agent_core::harness::pico3::session::CommitChanges`].
pub type TouchedDocs = HashSet<String>;
