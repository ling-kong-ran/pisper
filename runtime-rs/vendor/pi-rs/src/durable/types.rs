//! Port of `src/types.ts`: the shared durable record, query, write, and
//! document shapes, plus the `Storage` contract surface types.
//!
//! Wire fidelity (module divergence note D2): every serialized shape declares
//! its fields in the upstream construction order, and `Option` fields whose
//! upstream value is `undefined` are omitted from the JSON. Document values
//! are free-form `serde_json::Value` objects (upstream `JsonObject`), whose
//! member order `serde_json`'s `preserve_order` feature preserves.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ai::types::Message;
use crate::chord::delta::{op_from_json, op_to_json, Op};

pub use super::ids::{ConversationId, DocumentId, EntryId, Seq, SubmissionId, TaskId};

/// JSON object used as the root of every durable document (`types.ts`
/// `JsonObject`).
pub type JsonObject = serde_json::Map<String, Value>;

/// JSON value as accepted and stored by documents and record payloads
/// (upstream re-uses `@earendil-works/chord`'s `JsonValue`).
pub type JsonValue = Value;

/// How a waiting task treats the tasks it waits on (spec §5.5; `types.ts`
/// `JoinPolicy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JoinPolicy {
    FailFast,
    AllSettled,
}

/// Who owns a task: its conversation (a top-level task) or another task of the
/// same conversation (a child task) (`types.ts` `TaskOwnership`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TaskOwnership {
    Conversation,
    Task { task_id: TaskId },
}

/// Ownership selected explicitly whenever a conversation is created
/// (`types.ts` `ConversationOwnership`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ConversationOwnership {
    Ownerless,
    Task { task_id: TaskId },
}

/// Fork source and inclusive parent entry through which history is inherited
/// (`types.ts` `ConversationRecord.parent`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationParent {
    pub conversation_id: ConversationId,
    pub at: EntryId,
}

/// Creator edge used for attribution, subtree abort, and subtree idle waits
/// (`types.ts` `ConversationRecord.owner`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationOwner {
    pub conversation_id: ConversationId,
    pub task_id: TaskId,
}

/// Immutable identity, history ancestry, and task ownership of a transcript
/// scope (`types.ts` `ConversationRecord`). Construction order:
/// `{id, parent?, owner?}` (`session/transaction.ts` `#stageConversation`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationRecord {
    pub id: ConversationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ConversationParent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<ConversationOwner>,
}

/// An immutable override of one visible entry's contribution to model context
/// (`types.ts` `ContextEdit`). Construction order: `{target, action,
/// messages?}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextEdit {
    /// Entry whose model messages are omitted or replaced.
    pub target: EntryId,
    pub action: ContextEditAction,
    /// Messages contributed instead of the target entry's model messages
    /// (`action: "replace"` only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<Message>>,
}

/// `ContextEdit.action` (`types.ts:294-303`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContextEditAction {
    Omit,
    Replace,
}

/// The draft head spec: an explicit entry ID or `"self"` (`types.ts`
/// `EntryDraft.head`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryHead {
    Self_,
    Id(EntryId),
}

impl Serialize for EntryHead {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            EntryHead::Self_ => "self".serialize(serializer),
            EntryHead::Id(id) => id.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for EntryHead {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        match &value {
            Value::String(text) if text == "self" => Ok(EntryHead::Self_),
            Value::Number(number) => {
                let id = number
                    .as_i64()
                    .ok_or_else(|| serde::de::Error::custom("entry head must be an integer"))?;
                Ok(EntryHead::Id(id))
            }
            _ => Err(serde::de::Error::custom(
                "entry head must be \"self\" or an entry ID",
            )),
        }
    }
}

/// Entry content supplied before the Session assigns identity and task
/// attribution (`types.ts` `EntryDraft`). Key order at the single composition
/// site (`Tx#appendEntry`): draft keys, then `id`, `conversationId`, `head`,
/// `byTaskId` — mirrored by [`EntryRecord`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryDraft {
    /// Application-defined entry discriminator.
    pub kind: String,
    /// Messages contributed to model context; absent for display or
    /// bookkeeping entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<Vec<Message>>,
    /// JSON payload consumed by views, extensions, or bookkeeping logic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    /// Context-only overrides of earlier visible entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edits: Option<Vec<ContextEdit>>,
    /// `"self"` starts active context at the newly assigned entry ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<EntryHead>,
}

impl EntryDraft {
    /// A bare display/bookkeeping draft with only a kind.
    pub fn new(kind: impl Into<String>) -> Self {
        EntryDraft {
            kind: kind.into(),
            model: None,
            data: None,
            edits: None,
            head: None,
        }
    }
}

/// Immutable transcript event with separate model-facing and application-facing
/// payloads (`types.ts` `EntryRecord`). Serialization order matches
/// `Tx#appendEntry`'s `copyJson({...rest, id, conversationId, head,
/// byTaskId})`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryRecord {
    // Draft keys in `EntryDraft` order.
    /// Application-defined entry discriminator.
    pub kind: String,
    /// Messages contributed to model context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<Vec<Message>>,
    /// JSON payload consumed by views, extensions, or bookkeeping logic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    /// Context-only overrides of earlier visible entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edits: Option<Vec<ContextEdit>>,
    // Assigned identity keys.
    pub id: EntryId,
    pub conversation_id: ConversationId,
    /// First entry in the active context selected by this entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<EntryId>,
    /// Task that appended this entry, when it was produced by durable work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_task_id: Option<TaskId>,
}

impl EntryRecord {
    /// Apply an entry draft: assign `id`, `conversationId`, resolve `head`
    /// `"self"`, and stamp the appending task (`Tx#appendEntry`).
    pub fn from_draft(
        draft: EntryDraft,
        id: EntryId,
        conversation_id: ConversationId,
        by_task_id: Option<TaskId>,
    ) -> Self {
        let head = match draft.head {
            Some(EntryHead::Self_) => Some(id),
            Some(EntryHead::Id(head)) => Some(head),
            None => None,
        };
        EntryRecord {
            kind: draft.kind,
            model: draft.model,
            data: draft.data,
            edits: draft.edits,
            id,
            conversation_id,
            head,
            by_task_id,
        }
    }
}

/// Submission lifecycle type (`types.ts` `SubmissionRecord.type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SubmissionType {
    Input,
    Write,
}

/// Submission status (`types.ts` `SubmissionRecord.status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SubmissionStatus {
    Queued,
    Placed,
    Done,
    Unanswered,
}

/// Durable lifecycle of one admitted user input or passive entry write
/// (`types.ts` `SubmissionRecord`).
///
/// Wire key order (divergence note D2): upstream records evolve by object
/// spreads, so the serialized order is the creation order with settlement
/// fields appended. Two creation layouts exist in `harness/submissions.ts`:
/// queued (`{conversationId, requestId?, type, status}` + `id` appended by
/// `Tx.createSubmission`, settlement fields appending after the id) and
/// directly-settled writes/placed inputs (`{conversationId, requestId?, type,
/// status, entry}` + `id`, so `entry` precedes the id). [`SubmissionRecord`]
/// carries that choice in a skipped [`SubmissionRecord::entry_before_id`]
/// flag and hand-writes the wire form.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionRecord {
    pub conversation_id: ConversationId,
    /// Host-provided deduplication key, scoped to the conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub r#type: SubmissionType,
    pub status: SubmissionStatus,
    /// Entry contributed by the submission (`placed`/`done`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<EntryId>,
    /// Answering entry (`done` inputs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<EntryId>,
    /// Terminal reason (`unanswered`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Optional structured diagnostic data (`unanswered`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
    pub id: SubmissionId,
    /// Wire layout: `true` reproduces the directly-settled construction
    /// (`entry` before `id`); `false` the queued-then-settled one (`id`
    /// first). Not serialized.
    #[serde(skip, default)]
    pub entry_before_id: bool,
}

impl Serialize for SubmissionRecord {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serde_json::Map::new();
        map.insert(
            String::from("conversationId"),
            Value::from(self.conversation_id),
        );
        if let Some(request_id) = &self.request_id {
            map.insert(String::from("requestId"), Value::from(request_id.clone()));
        }
        map.insert(
            String::from("type"),
            Value::from(match self.r#type {
                SubmissionType::Input => "input",
                SubmissionType::Write => "write",
            }),
        );
        map.insert(
            String::from("status"),
            Value::from(match self.status {
                SubmissionStatus::Queued => "queued",
                SubmissionStatus::Placed => "placed",
                SubmissionStatus::Done => "done",
                SubmissionStatus::Unanswered => "unanswered",
            }),
        );
        let insert_entry = |map: &mut serde_json::Map<String, Value>| {
            if let Some(entry) = self.entry {
                map.insert(String::from("entry"), Value::from(entry));
            }
        };
        if self.entry_before_id {
            insert_entry(&mut map);
            map.insert(String::from("id"), Value::from(self.id));
        } else {
            map.insert(String::from("id"), Value::from(self.id));
            insert_entry(&mut map);
        }
        if let Some(answer) = self.answer {
            map.insert(String::from("answer"), Value::from(answer));
        }
        if let Some(reason) = &self.reason {
            map.insert(String::from("reason"), Value::from(reason.clone()));
        }
        if let Some(detail) = &self.detail {
            map.insert(String::from("detail"), detail.clone());
        }
        Value::Object(map).serialize(serializer)
    }
}

impl SubmissionRecord {
    /// A queued submission as created by `harness/submissions.ts` and stamped
    /// with its ID by `Tx.createSubmission`.
    pub fn queued(
        conversation_id: ConversationId,
        request_id: Option<String>,
        r#type: SubmissionType,
        id: SubmissionId,
    ) -> Self {
        SubmissionRecord {
            conversation_id,
            request_id,
            r#type,
            status: SubmissionStatus::Queued,
            entry: None,
            answer: None,
            reason: None,
            detail: None,
            id,
            entry_before_id: false,
        }
    }

    /// A directly-settled creation (`write` done with its entry, or a `placed`
    /// input): `entry` serializes before the id.
    pub fn settled_direct(
        conversation_id: ConversationId,
        request_id: Option<String>,
        r#type: SubmissionType,
        status: SubmissionStatus,
        entry: EntryId,
        id: SubmissionId,
    ) -> Self {
        SubmissionRecord {
            conversation_id,
            request_id,
            r#type,
            status,
            entry: Some(entry),
            answer: None,
            reason: None,
            detail: None,
            id,
            entry_before_id: true,
        }
    }
}

/// Terminal status staged for a submission; identity, type, and entry come
/// from its current record (`types.ts` `SubmissionSettlement`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SubmissionSettlement {
    Done {
        answer: EntryId,
    },
    Unanswered {
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<Value>,
    },
}

/// JSON-safe error snapshot persisted instead of a runtime `Error` object
/// (`types.ts` `TaskOutcomeError`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskOutcomeError {
    pub message: String,
    /// Optional structured diagnostic data for inspection or recovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
}

/// Durable reason and optional result recorded when a task becomes terminal
/// (`types.ts` `TaskOutcome`); tagged by `status` with upstream construction
/// order inside each variant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TaskOutcome {
    Completed {
        result: Value,
    },
    /// Expected task or domain failure explicitly committed by its
    /// implementation.
    Failed {
        error: TaskOutcomeError,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
    },
    /// Explicit cancellation handled by the task's abort protocol.
    Aborted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
    },
    /// Task that cannot resume because its definition or migration is
    /// unavailable.
    Orphaned {
        reason: String,
    },
    /// Runtime-detected contract failure, such as an uncaught throw or no
    /// durable progress.
    Faulted {
        error: TaskOutcomeError,
    },
}

/// Complete durable execution state of a task (`types.ts` `TaskState`); the
/// `status` tag leads every variant, matching `{status, ...}` construction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TaskState {
    /// Eligible for scheduling.
    Pending {
        /// Complete durable state from which execution resumes.
        checkpoint: Value,
    },
    /// Reserved by one in-memory task invocation.
    Running { checkpoint: Value },
    /// Parked without an invocation until every task in `on` is terminal; then
    /// resumes at `checkpoint`.
    Waiting {
        checkpoint: Value,
        on: Vec<TaskId>,
        policy: JoinPolicy,
    },
    /// Outcome decided; becomes terminal once no ordinary owned work below is
    /// live. Runs no more code.
    Completing { outcome: TaskOutcome },
    /// Permanently settled durable result receipt.
    Terminal { outcome: TaskOutcome },
}

impl TaskState {
    /// The `status` discriminant (`TaskQuery.status` /
    /// `TaskState.status`).
    pub fn status(&self) -> TaskStatus {
        match self {
            TaskState::Pending { .. } => TaskStatus::Pending,
            TaskState::Running { .. } => TaskStatus::Running,
            TaskState::Waiting { .. } => TaskStatus::Waiting,
            TaskState::Completing { .. } => TaskStatus::Completing,
            TaskState::Terminal { .. } => TaskStatus::Terminal,
        }
    }
}

/// `TaskState["status"]` discriminant values (`types.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    Pending,
    Running,
    Waiting,
    Completing,
    Terminal,
}

impl TaskStatus {
    /// The discriminant's wire text.
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Pending => "pending",
            TaskStatus::Running => "running",
            TaskStatus::Waiting => "waiting",
            TaskStatus::Completing => "completing",
            TaskStatus::Terminal => "terminal",
        }
    }
}

/// Complete replacement record for one durable task state machine (`types.ts`
/// `TaskRecord`). Wire order is the `Tx.createTask` literal
/// (`{id, conversationId, kind, version, input, owner?, background,
/// abortRequested, state}`) with `memos` appended later by spreads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRecord {
    pub id: TaskId,
    pub conversation_id: ConversationId,
    /// Registered task definition name.
    pub kind: String,
    /// Definition version used to migrate live input and checkpoints.
    pub version: i64,
    /// Original task input retained while the task is live or terminal.
    pub input: Value,
    /// Owning task of a child task; absent for a task its conversation owns.
    /// Immutable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<TaskId>,
    /// Whether this conversation-owned task is excluded from ordinary idle
    /// waits, conversation aborts, and cascades.
    pub background: bool,
    /// Durable abort mark checked before run-mode progress is committed.
    pub abort_requested: bool,
    pub state: TaskState,
    /// Small first-writer-wins values retained while the task can run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memos: Option<JsonObject>,
}

impl TaskRecord {
    /// The record's `state.status`.
    pub fn status(&self) -> TaskStatus {
        self.state.status()
    }
}

/// Creation options for a durable task (`types.ts` `TaskOptions`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskOptions {
    /// Required: a task always names its owner (spec §5.5).
    pub ownership: TaskOwnership,
    /// Default: the owner task's conversation, or the transaction's bound
    /// conversation; required for conversation-owned tasks created by Session
    /// commits that are not bound to a conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<ConversationId>,
    /// Conversation-owned tasks only: excluded from ordinary idle waits,
    /// conversation aborts, and cascades.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<bool>,
}

/// Document scope (`types.ts` `DocumentRecord.scope`); tagged by `kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DocumentScope {
    Session,
    Conversation { conversation_id: ConversationId },
    Task { task_id: TaskId },
}

/// History retention policy of a conversation document (`types.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DocumentHistory {
    /// Retain only current state.
    Latest,
    /// Retain history needed for as-of reads.
    Rewindable,
}

/// Fork initialization policy of a conversation document (`types.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DocumentFork {
    AsOf,
    Current,
    Initial,
}

/// Fields supplied when storage creates and stamps a new `DocumentRecord`
/// (`types.ts` `DocumentCreate`); wire order follows `documents.ts`
/// `documentCreate` (`{id, kind, key?, scope, history?, fork?}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentCreate {
    pub id: DocumentId,
    /// Stable document definition kind.
    pub kind: String,
    /// Family member key; absent for singleton documents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub scope: DocumentScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<DocumentHistory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork: Option<DocumentFork>,
}

/// Persisted lifecycle record for one create-to-retire document incarnation
/// (`types.ts` `DocumentRecord`); wire order is
/// `{...create, createdAt, retiredAt?}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentRecord {
    pub id: DocumentId,
    /// Stable document definition kind.
    pub kind: String,
    /// Family member key; absent for singleton documents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub scope: DocumentScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<DocumentHistory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork: Option<DocumentFork>,
    /// Commit that created the incarnation, stamped by storage.
    pub created_at: Seq,
    /// Commit that retired the incarnation; absent while it is current.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<Seq>,
}

impl DocumentRecord {
    /// Stamp a create with its creation (and optional retirement) commit.
    pub fn from_create(create: DocumentCreate, created_at: Seq, retired_at: Option<Seq>) -> Self {
        DocumentRecord {
            id: create.id,
            kind: create.kind,
            key: create.key,
            scope: create.scope,
            history: create.history,
            fork: create.fork,
            created_at,
            retired_at,
        }
    }
}

/// `Vec<Op>` with the upstream tuple wire form (chord's `op_to_json` /
/// `op_from_json`), because `chord::delta::Op` deliberately does not implement
/// `serde` traits itself.
#[derive(Debug, Clone, PartialEq)]
pub struct OpList(pub Vec<Op>);

impl Serialize for OpList {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let wire: Vec<Value> = self.0.iter().map(op_to_json).collect();
        wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for OpList {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = Vec::<Value>::deserialize(deserializer)?;
        let ops = wire
            .iter()
            .map(op_from_json)
            .collect::<Result<Vec<Op>, _>>()
            .map_err(|error| serde::de::Error::custom(error.message()))?;
        Ok(OpList(ops))
    }
}

/// Complete checkpoint or Chord operation batch selected by the owning Session
/// (`types.ts` `DocumentContent`). Construction is
/// `{version, kind: "base"|"delta", value|ops}` — `version` first — so the
/// port hand-writes the wire form.
#[derive(Debug, Clone, PartialEq)]
pub enum DocumentContent {
    Base { version: i64, value: JsonObject },
    Delta { version: i64, ops: Vec<Op> },
}

impl Serialize for DocumentContent {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serde_json::Map::new();
        match self {
            DocumentContent::Base { version, value } => {
                map.insert("version".into(), Value::from(*version));
                map.insert("kind".into(), Value::from("base"));
                map.insert("value".into(), Value::Object(value.clone()));
            }
            DocumentContent::Delta { version, ops } => {
                map.insert("version".into(), Value::from(*version));
                map.insert("kind".into(), Value::from("delta"));
                let wire: Vec<Value> = ops.iter().map(op_to_json).collect();
                map.insert("ops".into(), Value::Array(wire));
            }
        }
        Value::Object(map).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DocumentContent {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let object = value
            .as_object()
            .ok_or_else(|| serde::de::Error::custom("document content must be an object"))?;
        let version = object
            .get("version")
            .and_then(Value::as_i64)
            .filter(|version| *version >= 1)
            .ok_or_else(|| {
                serde::de::Error::custom("document content requires a positive version")
            })?;
        match object.get("kind").and_then(Value::as_str) {
            Some("base") => {
                let value = object
                    .get("value")
                    .and_then(Value::as_object)
                    .ok_or_else(|| {
                        serde::de::Error::custom("base content requires a value object")
                    })?
                    .clone();
                Ok(DocumentContent::Base { version, value })
            }
            Some("delta") => {
                let ops = object.get("ops").and_then(Value::as_array).ok_or_else(|| {
                    serde::de::Error::custom("delta content requires an ops array")
                })?;
                let ops = ops
                    .iter()
                    .map(op_from_json)
                    .collect::<Result<Vec<Op>, _>>()
                    .map_err(|error| serde::de::Error::custom(error.message()))?;
                Ok(DocumentContent::Delta { version, ops })
            }
            _ => Err(serde::de::Error::custom(
                "document content requires kind base or delta",
            )),
        }
    }
}

/// One record or document mutation in an atomic storage commit (`types.ts`
/// `StorageWrite`); tagged by `type` with `{type, ...}` construction order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum StorageWrite {
    #[serde(rename = "conversation")]
    Conversation { value: ConversationRecord },
    #[serde(rename = "entry")]
    Entry { value: EntryRecord },
    #[serde(rename = "task")]
    Task { value: TaskRecord },
    #[serde(rename = "submission")]
    Submission { value: SubmissionRecord },
    #[serde(rename = "document.create")]
    DocumentCreate {
        record: DocumentCreate,
        content: DocumentContent,
    },
    #[serde(rename = "document.copy")]
    DocumentCopy {
        record: DocumentCreate,
        source: DocumentCopySource,
    },
    #[serde(rename = "document.change")]
    DocumentChange {
        id: DocumentId,
        content: DocumentContent,
    },
    #[serde(rename = "document.retire")]
    DocumentRetire { id: DocumentId },
}

/// The table-record subset of [`StorageWrite`] (`types.ts`
/// `TableCommitChange`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TableCommitChange {
    #[serde(rename = "conversation")]
    Conversation { value: ConversationRecord },
    #[serde(rename = "entry")]
    Entry { value: EntryRecord },
    #[serde(rename = "task")]
    Task { value: TaskRecord },
    #[serde(rename = "submission")]
    Submission { value: SubmissionRecord },
}

/// Exact persisted source selected for a definition-free document copy
/// (`types.ts` `DocumentCopySource`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentCopySource {
    pub id: DocumentId,
    pub at: DocumentPoint,
}

/// Current state or one historical commit sequence used for document
/// membership and content reads (`types.ts` `DocumentPoint`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentPoint {
    Current,
    Seq(Seq),
}

impl Serialize for DocumentPoint {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            DocumentPoint::Current => "current".serialize(serializer),
            DocumentPoint::Seq(seq) => seq.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for DocumentPoint {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        match &value {
            Value::String(text) if text == "current" => Ok(DocumentPoint::Current),
            Value::Number(number) => {
                let seq = number
                    .as_i64()
                    .ok_or_else(|| serde::de::Error::custom("document point must be an integer"))?;
                Ok(DocumentPoint::Seq(seq))
            }
            _ => Err(serde::de::Error::custom(
                "document point must be \"current\" or a sequence",
            )),
        }
    }
}

/// Exact logical identity of a singleton or one keyed family member
/// (`types.ts` `DocumentAddress`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentAddress {
    pub kind: String,
    pub scope: DocumentScope,
    /// Absent selects the singleton; present selects one family member.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// Detached materialized value and stored definition version at a selected
/// point (`types.ts` `StoredDocument`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredDocument {
    pub record: DocumentRecord,
    pub version: i64,
    pub value: JsonObject,
    /// Deltas replayed after the selected base to materialize `value`.
    pub deltas_since_base: i64,
}

/// Committed change of one document incarnation (`types.ts`
/// `DocumentCommitChange`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DocumentCommitChange {
    #[serde(rename = "document")]
    Document {
        record: DocumentRecord,
        /// Conversation owning the document; task documents derive it from
        /// their task record. `None` only for Session documents.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation_id: Option<ConversationId>,
        /// Definition version of `value`; `None` when this commit retired the
        /// incarnation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<i64>,
        /// Exact adopted immutable revision, or `null` when this commit
        /// retired the incarnation (serialized as JSON `null`, never
        /// omitted).
        value: Value,
        /// Exact adopted operations for an ordinary update; empty for creation
        /// and retirement.
        ops: OpList,
    },
    /// Definition-free child initialization; consumers hydrate through state
    /// or watch acquisition.
    #[serde(rename = "document.copy")]
    DocumentCopy {
        record: DocumentRecord,
        conversation_id: ConversationId,
        source: DocumentCopySource,
    },
}

/// Every immutable change from one successful Session commit. Change order is
/// unspecified (`types.ts` `CommitChange` / `CommitPublication`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CommitChange {
    Table(TableCommitChange),
    Document(DocumentCommitChange),
}

/// Every immutable change from one successful Session commit (`types.ts`
/// `CommitPublication`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitPublication {
    pub seq: Seq,
    pub changes: Vec<CommitChange>,
}

/// Backend-owned JSON continuation state that callers only round-trip to the
/// same scan (`types.ts` `Cursor`).
pub type Cursor = JsonObject;

/// One ordered scan result and its optional continuation state (`types.ts`
/// `Page<T, C>`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub items: Vec<T>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<Cursor>,
}

impl<T> Page<T> {
    pub fn empty() -> Self {
        Page {
            items: Vec::new(),
            next: None,
        }
    }
}

/// Optional filters for an ordered conversation scan (`types.ts`
/// `ConversationQuery`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_conversation_id: Option<ConversationId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_task_id: Option<TaskId>,
}

/// Inclusive ID bounds for a newest-first scan of one conversation's
/// fork-aware history (`types.ts` `EntryQuery`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryQuery {
    pub conversation_id: ConversationId,
    /// Oldest entry ID that may be returned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_entry_id: Option<EntryId>,
    /// Newest entry ID that may be returned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_entry_id: Option<EntryId>,
}

/// Optional filters for an ordered scan of durable task records (`types.ts`
/// `TaskQuery`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<ConversationId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TaskStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abort_requested: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<bool>,
}

/// Optional filters for an ordered scan of submission records (`types.ts`
/// `SubmissionQuery`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<ConversationId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<SubmissionStatus>,
}

/// Ordered scan of document incarnations alive in one exact scope at one point
/// (`types.ts` `DocumentQuery`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentQuery {
    pub scope: DocumentScope,
    pub at: DocumentPoint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// Stored replay state supplied to a document's checkpoint predicate
/// (`types.ts` `CheckpointInfo`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointInfo {
    /// Deltas already stored after the newest base, excluding the change being
    /// evaluated.
    pub deltas_since_base: i64,
}

/// Terminal status of one document watch (`types.ts` `WatchEnd`).
#[derive(Debug, Clone)]
pub enum WatchEnd {
    Reason(WatchEndReason),
    ListenerError {
        error: Arc<dyn std::error::Error + Send + Sync>,
    },
}

/// `WatchEnd` reason strings (`types.ts:828`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchEndReason {
    Stopped,
    Cancelled,
    SessionClosed,
    Retired,
}

impl fmt::Display for WatchEndReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            WatchEndReason::Stopped => "stopped",
            WatchEndReason::Cancelled => "cancelled",
            WatchEndReason::SessionClosed => "session_closed",
            WatchEndReason::Retired => "retired",
        };
        f.write_str(text)
    }
}
