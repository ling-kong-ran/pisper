//! Port of `src/storage/memory.ts`: the detached in-memory reference
//! implementation of [`Storage`].
//!
//! Reads and retained writes are cloned intentionally to match the ownership
//! boundary of serialization-backed stores. This is backend conformance, not
//! validation.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde_json::Value;

use crate::agent_core::chord_support::context::Context;
use crate::chord::delta::apply_immutable_batches;

use super::{EntryWithCommitSeq, Storage, StorageError, StorageErrorKind};
use crate::durable::errors::StorageRejected;
use crate::durable::types::{
    ConversationQuery, ConversationRecord, Cursor, DocumentAddress, DocumentContent,
    DocumentCreate, DocumentHistory, DocumentPoint, DocumentQuery, DocumentRecord, DocumentScope,
    EntryQuery, EntryRecord, Page, StorageWrite, StoredDocument, SubmissionQuery, SubmissionRecord,
    TaskId, TaskQuery, TaskRecord, TaskStatus,
};

type StoredTask = TaskRecord;
type SubmissionStatus = crate::durable::types::SubmissionStatus;

/// One persisted document revision (upstream `DocumentRevision`).
#[derive(Debug, Clone)]
struct DocumentRevision {
    content: DocumentContent,
    seq: i64,
}

/// Stored document state (upstream `StoredDocumentState`).
#[derive(Debug, Clone)]
struct StoredDocumentState {
    record: DocumentRecord,
    revisions: Vec<DocumentRevision>,
}

/// One document mutation in a batch (upstream `DocumentAction`).
#[derive(Debug, Default, Clone)]
struct DocumentAction {
    create: Option<DocumentCreate>,
    content: Option<DocumentContent>,
    retire: bool,
}

/// Which table owns a global ID (upstream `TableName`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TableName {
    Conversation,
    Entry,
    Task,
    Submission,
    Document,
}

/// Deterministic string identity of one document scope
/// (`memory.ts` `scopeKey`, via `JSON.stringify`).
fn scope_key(scope: &DocumentScope) -> String {
    let array: Vec<Value> = match scope {
        DocumentScope::Session => vec![Value::from("session")],
        DocumentScope::Conversation { conversation_id } => {
            vec![Value::from("conversation"), Value::from(*conversation_id)]
        }
        DocumentScope::Task { task_id } => vec![Value::from("task"), Value::from(*task_id)],
    };
    Value::Array(array).to_string()
}

/// Deterministic string identity of one logical address
/// (`memory.ts` `addressKey`).
fn address_key(address: &DocumentAddress) -> String {
    let key = match &address.key {
        Some(key) => vec![Value::from("family"), Value::from(key.clone())],
        None => vec![Value::from("singleton")],
    };
    let array = vec![
        Value::from(address.kind.clone()),
        Value::from(scope_key(&address.scope)),
        Value::Array(key),
    ];
    Value::Array(array).to_string()
}

/// `recordAddressKey(record)` (`memory.ts:186-187`).
fn record_address_key(record: &DocumentRecord) -> String {
    address_key(&DocumentAddress {
        kind: record.kind.clone(),
        scope: record.scope,
        key: record.key.clone(),
    })
}

/// `isAliveAt(record, at)` (`memory.ts:189-192`).
fn is_alive_at(record: &DocumentRecord, at: DocumentPoint) -> bool {
    match at {
        DocumentPoint::Current => record.retired_at.is_none(),
        DocumentPoint::Seq(at) => {
            record.created_at <= at
                && (record.retired_at.is_none() || at < record.retired_at.unwrap())
        }
    }
}

/// `isCurrentOnly(record)` (`memory.ts:194-196`).
fn is_current_only(record: &DocumentRecord) -> bool {
    !matches!(record.scope, DocumentScope::Conversation { .. })
        || record.history == Some(DocumentHistory::Latest)
}

/// A fully validated, detached state mutation whose application performs no
/// fallible preparation (`memory.ts` `PreparedMemoryCommit`).
pub struct PreparedMemoryCommit {
    pub seq: i64,
    /// Deeply detached writes for persistence.
    pub writes: Vec<StorageWrite>,
    document_actions: Vec<(i64, DocumentAction)>,
    applied: bool,
}

/// Mutable index state, behind one mutex for `&self` trait access.
struct State {
    record_types: HashMap<i64, TableName>,
    conversations: HashMap<i64, ConversationRecord>,
    conversation_ids: Vec<i64>,
    conversation_ids_by_owner_conversation: HashMap<i64, Vec<i64>>,
    conversation_ids_by_owner_task: HashMap<i64, Vec<i64>>,
    entries: HashMap<i64, EntryRecord>,
    entry_ids: HashMap<i64, Vec<i64>>,
    head_entry_ids: HashMap<i64, Vec<i64>>,
    entry_commit_seqs: HashMap<i64, i64>,
    tasks: HashMap<i64, StoredTask>,
    task_ids: Vec<i64>,
    task_ids_by_status: [Vec<i64>; 5],
    submissions: HashMap<i64, SubmissionRecord>,
    submission_ids: Vec<i64>,
    submission_ids_by_status: [Vec<i64>; 4],
    submission_ids_by_request: HashMap<i64, HashMap<String, i64>>,
    documents: HashMap<i64, StoredDocumentState>,
    document_addresses: HashMap<String, AddressIndex>,
    document_ids_by_scope: HashMap<String, Vec<i64>>,
}

#[derive(Debug, Default, Clone)]
struct AddressIndex {
    ids: Vec<i64>,
    current_id: Option<i64>,
}

impl State {
    fn new() -> Self {
        State {
            record_types: HashMap::new(),
            conversations: HashMap::new(),
            conversation_ids: Vec::new(),
            conversation_ids_by_owner_conversation: HashMap::new(),
            conversation_ids_by_owner_task: HashMap::new(),
            entries: HashMap::new(),
            entry_ids: HashMap::new(),
            head_entry_ids: HashMap::new(),
            entry_commit_seqs: HashMap::new(),
            tasks: HashMap::new(),
            task_ids: Vec::new(),
            task_ids_by_status: [Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            submissions: HashMap::new(),
            submission_ids: Vec::new(),
            submission_ids_by_status: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            submission_ids_by_request: HashMap::new(),
            documents: HashMap::new(),
            document_addresses: HashMap::new(),
            document_ids_by_scope: HashMap::new(),
        }
    }
}

fn task_status_slot(status: TaskStatus) -> usize {
    match status {
        TaskStatus::Pending => 0,
        TaskStatus::Running => 1,
        TaskStatus::Waiting => 2,
        TaskStatus::Completing => 3,
        TaskStatus::Terminal => 4,
    }
}

fn submission_status_slot(status: SubmissionStatus) -> usize {
    match status {
        SubmissionStatus::Queued => 0,
        SubmissionStatus::Placed => 1,
        SubmissionStatus::Done => 2,
        SubmissionStatus::Unanswered => 3,
    }
}

/// `cursorId(cursor)` (`memory.ts:120-125`).
fn cursor_id(cursor: Option<&Cursor>) -> Result<Option<i64>, StorageError> {
    let Some(cursor) = cursor else {
        return Ok(None);
    };
    let Some(after) = cursor.get("after") else {
        return Ok(None);
    };
    let Some(after) = after.as_i64() else {
        return Err(StorageError::generic("Invalid storage cursor"));
    };
    Ok(Some(after))
}

/// `lowerBound(ids, target)` (`memory.ts:127-136`).
fn lower_bound(ids: &[i64], target: i64) -> usize {
    let mut low = 0usize;
    let mut high = ids.len();
    while low < high {
        let middle = (low + high) / 2;
        if ids[middle] < target {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    low
}

/// `upperBound(ids, target)` (`memory.ts:138-147`).
fn upper_bound(ids: &[i64], target: i64) -> usize {
    let mut low = 0usize;
    let mut high = ids.len();
    while low < high {
        let middle = (low + high) / 2;
        if ids[middle] <= target {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    low
}

/// `insertSorted(ids, id)` (`memory.ts:149-152`).
fn insert_sorted(ids: &mut Vec<i64>, id: i64) {
    if ids.is_empty() || ids[ids.len() - 1] < id {
        ids.push(id);
    } else {
        ids.insert(lower_bound(ids, id), id);
    }
}

/// `removeSorted(ids, id)` (`memory.ts:154-157`).
fn remove_sorted(ids: &mut Vec<i64>, id: i64) {
    let index = lower_bound(ids, id);
    if ids.get(index) == Some(&id) {
        ids.remove(index);
    }
}

/// `insertMapId(index, key, id)` (`memory.ts:159-166`).
fn insert_map_id<V>(index: &mut HashMap<V, Vec<i64>>, key: V, id: i64)
where
    V: std::hash::Hash + Eq + Clone,
{
    let ids = index.entry(key).or_default();
    insert_sorted(ids, id);
}

/// `page(values, limit)` (`memory.ts:199-203`).
fn page<T: Clone + HasId>(values: &[T], limit: usize) -> Page<T> {
    let items: Vec<T> = values.iter().take(limit).cloned().collect();
    if values.len() <= limit {
        return Page { items, next: None };
    }
    let after = Value::from(items[items.len() - 1].id());
    let mut next = serde_json::Map::new();
    next.insert(String::from("after"), after);
    Page {
        items,
        next: Some(next),
    }
}

/// The `{ id }` read `page()` needs for its continuation cursor.
trait HasId {
    fn id(&self) -> i64;
}

impl HasId for ConversationRecord {
    fn id(&self) -> i64 {
        self.id
    }
}

impl HasId for EntryRecord {
    fn id(&self) -> i64 {
        self.id
    }
}

impl HasId for TaskRecord {
    fn id(&self) -> i64 {
        self.id
    }
}

impl HasId for SubmissionRecord {
    fn id(&self) -> i64 {
        self.id
    }
}

impl HasId for DocumentRecord {
    fn id(&self) -> i64 {
        self.id
    }
}

/// Detached in-memory reference implementation of `Storage`
/// (`memory.ts` `MemoryStorage`).
pub struct MemoryStorage {
    state: Mutex<State>,
    next_id: Mutex<i64>,
    next_seq: Mutex<i64>,
    closed: AtomicBool,
}

impl Default for MemoryStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryStorage {
    pub fn new() -> Self {
        MemoryStorage {
            state: Mutex::new(State::new()),
            next_id: Mutex::new(2),
            next_seq: Mutex::new(1),
            closed: AtomicBool::new(false),
        }
    }

    fn assert_open_error() -> StorageError {
        StorageError::closed("MemoryStorage is closed")
    }

    fn assert_open(&self) -> Result<(), StorageError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(Self::assert_open_error());
        }
        Ok(())
    }

    /// Lock the index state after the liveness check every upstream operation
    /// performs (`memory.ts` `assertOpen`).
    fn read_state(&self) -> Result<std::sync::MutexGuard<'_, State>, StorageError> {
        self.assert_open()?;
        Ok(self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()))
    }

    /// Validate and detach one commit without changing observable state
    /// (`memory.ts` `prepareCommit`).
    pub fn prepare_commit(
        &self,
        writes: &[StorageWrite],
        seq: Option<i64>,
    ) -> Result<PreparedMemoryCommit, StorageError> {
        self.assert_open()?;
        let next_seq = *self
            .next_seq
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let seq = seq.unwrap_or(next_seq);
        if seq < next_seq {
            return Err(StorageError::generic(format!(
                "Commit sequence {seq} does not strictly increase"
            )));
        }
        let resolved = self.resolve_document_copies(writes.to_vec())?;
        self.check_global_ids(&resolved)?;
        let document_actions = self.prepare_document_actions(&resolved)?;
        self.check_document_actions(&document_actions)?;
        Ok(PreparedMemoryCommit {
            seq,
            writes: resolved,
            document_actions,
            applied: false,
        })
    }

    /// Value-typed entry used by the JSONL recovery path: convert validated
    /// raw write objects to [`StorageWrite`] and prepare. Conversion failures
    /// surface with the caller's corruption mapping.
    pub fn prepare_commit_value(
        &self,
        writes: &[serde_json::Value],
        seq: Option<i64>,
    ) -> Result<PreparedMemoryCommit, StorageError> {
        self.assert_open()?;
        let mut typed: Vec<StorageWrite> = Vec::with_capacity(writes.len());
        for write in writes {
            typed.push(
                serde_json::from_value::<StorageWrite>(write.clone()).map_err(|error| {
                    StorageError::generic(format!(
                        "write does not match the storage write schema: {error}"
                    ))
                })?,
            );
        }
        self.prepare_commit(&typed, seq)
    }

    /// Apply a prepared commit exactly once (`PreparedMemoryCommit.apply`).
    pub fn apply_prepared(&self, prepared: &mut PreparedMemoryCommit) -> Result<i64, StorageError> {
        self.assert_open()?;
        if prepared.applied {
            return Ok(prepared.seq);
        }
        prepared.applied = true;
        self.apply_prepared_inner(&prepared.writes, &prepared.document_actions, prepared.seq)?;
        Ok(prepared.seq)
    }

    /// `resolveDocumentCopies` (`memory.ts:273-311`).
    fn resolve_document_copies(
        &self,
        writes: Vec<StorageWrite>,
    ) -> Result<Vec<StorageWrite>, StorageError> {
        if !writes
            .iter()
            .any(|write| matches!(write, StorageWrite::DocumentCopy { .. }))
        {
            return Ok(writes);
        }
        let mut changed_document_ids: Vec<i64> = Vec::new();
        for write in &writes {
            match write {
                StorageWrite::DocumentCreate { record, .. }
                | StorageWrite::DocumentCopy { record, .. } => {
                    changed_document_ids.push(record.id);
                }
                StorageWrite::DocumentChange { id, .. } | StorageWrite::DocumentRetire { id } => {
                    changed_document_ids.push(*id);
                }
                _ => {}
            }
        }
        let mut resolved = Vec::with_capacity(writes.len());
        for write in writes {
            let StorageWrite::DocumentCopy { record, source } = write else {
                resolved.push(write);
                continue;
            };
            let outcome = self.resolve_one_copy(&record, &source, &changed_document_ids);
            match outcome {
                Ok(write) => resolved.push(write),
                Err(error) => {
                    if error.kind == StorageErrorKind::Rejected {
                        return Err(error);
                    }
                    return Err(StorageError::rejected(format!(
                        "Document copy {} was rejected",
                        record.id
                    )));
                }
            }
        }
        Ok(resolved)
    }

    /// One `document.copy` resolution arm of `resolveDocumentCopies`
    /// (`memory.ts:285-309`).
    fn resolve_one_copy(
        &self,
        record: &DocumentCreate,
        source: &crate::durable::types::DocumentCopySource,
        changed_document_ids: &[i64],
    ) -> Result<StorageWrite, StorageError> {
        if changed_document_ids.contains(&source.id) {
            return Err(StorageError::generic(format!(
                "Fork source document {} is changed in the copy batch",
                source.id
            )));
        }
        let stored = self
            .materialize_document(source.id, source.at)?
            .ok_or_else(|| {
                StorageError::generic(format!("Fork source document {} cannot be read", source.id))
            })?;
        if !matches!(stored.record.scope, DocumentScope::Conversation { .. })
            || !matches!(record.scope, DocumentScope::Conversation { .. })
            || stored.record.kind != record.kind
            || stored.record.key != record.key
            || stored.record.history != record.history
            || stored.record.fork != record.fork
        {
            return Err(StorageError::generic(format!(
                "Fork source document {} does not match the copied record",
                source.id
            )));
        }
        Ok(StorageWrite::DocumentCreate {
            record: record.clone(),
            content: DocumentContent::Base {
                version: stored.version,
                value: stored.value,
            },
        })
    }

    /// `applyPreparedCommit` (`memory.ts:313-408`).
    fn apply_prepared_inner(
        &self,
        prepared: &[StorageWrite],
        document_actions: &[(i64, DocumentAction)],
        seq: i64,
    ) -> Result<i64, StorageError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut next_id = self
            .next_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for write in prepared {
            match write {
                StorageWrite::Conversation { value } => {
                    state.record_types.insert(value.id, TableName::Conversation);
                    state.conversations.insert(value.id, value.clone());
                    insert_sorted(&mut state.conversation_ids, value.id);
                    if let Some(owner) = &value.owner {
                        insert_map_id(
                            &mut state.conversation_ids_by_owner_conversation,
                            owner.conversation_id,
                            value.id,
                        );
                        insert_map_id(
                            &mut state.conversation_ids_by_owner_task,
                            owner.task_id,
                            value.id,
                        );
                    }
                    *next_id = (*next_id).max(value.id + 1);
                }
                StorageWrite::Entry { value } => {
                    state.record_types.insert(value.id, TableName::Entry);
                    state.entries.insert(value.id, value.clone());
                    state.entry_commit_seqs.insert(value.id, seq);
                    insert_map_id(&mut state.entry_ids, value.conversation_id, value.id);
                    if value.head.is_some() {
                        insert_map_id(&mut state.head_entry_ids, value.conversation_id, value.id);
                    }
                    *next_id = (*next_id).max(value.id + 1);
                }
                StorageWrite::Task { value } => {
                    state.record_types.insert(value.id, TableName::Task);
                    let previous = state.tasks.get(&value.id).map(|task| task.status());
                    if previous.is_none() {
                        insert_sorted(&mut state.task_ids, value.id);
                        insert_sorted(
                            &mut state.task_ids_by_status[task_status_slot(value.status())],
                            value.id,
                        );
                    } else if let Some(previous) =
                        previous.filter(|previous| *previous != value.status())
                    {
                        remove_sorted(
                            &mut state.task_ids_by_status[task_status_slot(previous)],
                            value.id,
                        );
                        insert_sorted(
                            &mut state.task_ids_by_status[task_status_slot(value.status())],
                            value.id,
                        );
                    }
                    state.tasks.insert(value.id, value.clone());
                    *next_id = (*next_id).max(value.id + 1);
                }
                StorageWrite::Submission { value } => {
                    state.record_types.insert(value.id, TableName::Submission);
                    let previous = state.submissions.get(&value.id);
                    let previous_status = previous.map(|submission| submission.status);
                    let previous_request = previous.and_then(|submission| {
                        submission
                            .request_id
                            .clone()
                            .map(|request_id| (submission.conversation_id, request_id))
                    });
                    if previous.is_none() {
                        insert_sorted(&mut state.submission_ids, value.id);
                        insert_sorted(
                            &mut state.submission_ids_by_status
                                [submission_status_slot(value.status)],
                            value.id,
                        );
                    } else if previous_status != Some(value.status) {
                        remove_sorted(
                            &mut state.submission_ids_by_status
                                [submission_status_slot(previous_status.unwrap())],
                            value.id,
                        );
                        insert_sorted(
                            &mut state.submission_ids_by_status
                                [submission_status_slot(value.status)],
                            value.id,
                        );
                    }
                    if let Some((previous_conversation_id, previous_request_id)) = previous_request
                    {
                        if let Some(previous_requests) = state
                            .submission_ids_by_request
                            .get_mut(&previous_conversation_id)
                        {
                            if previous_requests.get(&previous_request_id) == Some(&value.id) {
                                previous_requests.remove(&previous_request_id);
                                if previous_requests.is_empty() {
                                    state
                                        .submission_ids_by_request
                                        .remove(&previous_conversation_id);
                                }
                            }
                        }
                    }
                    state.submissions.insert(value.id, value.clone());
                    if let Some(request_id) = &value.request_id {
                        let requests = state
                            .submission_ids_by_request
                            .entry(value.conversation_id)
                            .or_default();
                        requests.insert(request_id.clone(), value.id);
                    }
                    *next_id = (*next_id).max(value.id + 1);
                }
                StorageWrite::DocumentCopy { .. } => {
                    return Err(StorageError::generic(
                        "Prepared document copy was not resolved",
                    ));
                }
                StorageWrite::DocumentCreate { .. }
                | StorageWrite::DocumentChange { .. }
                | StorageWrite::DocumentRetire { .. } => {}
            }
        }
        drop(next_id);
        self.apply_document_actions(&mut state, document_actions, seq);
        *self
            .next_seq
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = seq + 1;
        Ok(seq)
    }

    /// `materializeDocument` (`memory.ts:635-652`).
    fn materialize_document(
        &self,
        id: i64,
        at: DocumentPoint,
    ) -> Result<Option<StoredDocument>, StorageError> {
        let state = self.read_state()?;
        let Some(stored) = state.documents.get(&id) else {
            return Ok(None);
        };
        if at != DocumentPoint::Current && is_current_only(&stored.record) {
            return Err(StorageError::generic(format!(
                "Document {id} does not retain historical content"
            )));
        }
        if !is_alive_at(&stored.record, at) {
            return Ok(None);
        }
        let revisions: Vec<DocumentRevision> = match at {
            DocumentPoint::Current => stored.revisions.clone(),
            DocumentPoint::Seq(at) => stored
                .revisions
                .iter()
                .filter(|revision| revision.seq <= at)
                .cloned()
                .collect(),
        };
        let mut base_index = revisions.len() as i64 - 1;
        while base_index >= 0
            && !matches!(
                revisions[base_index as usize].content,
                DocumentContent::Base { .. }
            )
        {
            base_index -= 1;
        }
        if base_index < 0 {
            return Err(StorageError::generic(format!(
                "Document {id} is missing a required base"
            )));
        }
        let base_index = base_index as usize;
        let base = &revisions[base_index];
        let DocumentContent::Base { version, value } = &base.content else {
            return Err(StorageError::generic(format!(
                "Document {id} is missing a required base"
            )));
        };
        let mut batches: Vec<Vec<crate::chord::delta::Op>> = Vec::new();
        for revision in &revisions[base_index + 1..] {
            match &revision.content {
                DocumentContent::Delta {
                    version: delta_version,
                    ops,
                } if *delta_version == *version => {
                    batches.push(ops.clone());
                }
                _ => {
                    return Err(StorageError::generic(format!(
                        "Document {id} crosses a stored version boundary without a base"
                    )));
                }
            }
        }
        let value = apply_immutable_batches(Some(&Value::Object(value.clone())), batches)
            .map_err(|error| StorageError::generic(error.message()))?;
        let Some(value) = value.as_object().cloned() else {
            return Err(StorageError::generic(format!(
                "Document {id} is missing a required base"
            )));
        };
        Ok(Some(StoredDocument {
            record: stored.record.clone(),
            version: *version,
            value,
            deltas_since_base: (revisions.len() - base_index - 1) as i64,
        }))
    }

    /// `visibleEntries` (`memory.ts:654-677`): walk newest-first through the
    /// conversation's ancestry. `visit` returns `false` to stop; the closure
    /// reports whether iteration ended early.
    fn visit_visible_entries<F>(
        &self,
        conversation_id: i64,
        min_entry_id: i64,
        max_entry_id: Option<i64>,
        visit: &mut F,
    ) -> Result<(), StorageError>
    where
        F: FnMut(&EntryRecord) -> bool,
    {
        let state = self.read_state()?;
        if !state.conversations.contains_key(&conversation_id) {
            return Err(StorageError::generic(format!(
                "Unknown conversation: {conversation_id}"
            )));
        }
        let mut current_id = conversation_id;
        let mut upper_entry_id = max_entry_id;
        loop {
            let ids = state
                .entry_ids
                .get(&current_id)
                .cloned()
                .unwrap_or_default();
            let start = match upper_entry_id {
                Some(upper) => upper_bound(&ids, upper),
                None => ids.len(),
            };
            for index in (0..start).rev() {
                let id = ids[index];
                if id < min_entry_id {
                    return Ok(());
                }
                if !visit(state.entries.get(&id).unwrap()) {
                    return Ok(());
                }
            }
            let conversation = state.conversations.get(&current_id).unwrap();
            let Some(parent) = conversation.parent else {
                return Ok(());
            };
            upper_entry_id = Some(match upper_entry_id {
                Some(upper) => upper.min(parent.at),
                None => parent.at,
            });
            if let Some(upper) = upper_entry_id {
                if upper < min_entry_id {
                    return Ok(());
                }
            }
            current_id = parent.conversation_id;
        }
    }

    /// `checkGlobalIds` (`memory.ts:679-699`).
    fn check_global_ids(&self, writes: &[StorageWrite]) -> Result<(), StorageError> {
        let state = self.read_state()?;
        let mut claimed: HashMap<i64, TableName> = HashMap::new();
        for write in writes {
            let (table, id) = match write {
                StorageWrite::Conversation { value } => (TableName::Conversation, value.id),
                StorageWrite::Entry { value } => (TableName::Entry, value.id),
                StorageWrite::Task { value } => (TableName::Task, value.id),
                StorageWrite::Submission { value } => (TableName::Submission, value.id),
                StorageWrite::DocumentCreate { record, .. }
                | StorageWrite::DocumentCopy { record, .. } => (TableName::Document, record.id),
                StorageWrite::DocumentChange { .. } | StorageWrite::DocumentRetire { .. } => {
                    continue
                }
            };
            let existing = state.record_types.get(&id).copied();
            let earlier = claimed.get(&id).copied();
            match table {
                TableName::Conversation | TableName::Entry | TableName::Document => {
                    if let Some(existing) = existing {
                        return Err(StorageError::generic(format!(
                            "ID {id} already belongs to {}",
                            table_name(existing)
                        )));
                    }
                    if earlier.is_some() {
                        return Err(StorageError::generic(format!(
                            "ID {id} is written more than once"
                        )));
                    }
                }
                TableName::Task | TableName::Submission => {
                    if existing.is_some_and(|existing| existing != table) {
                        return Err(StorageError::generic(format!(
                            "ID {id} already belongs to {}",
                            table_name(existing.unwrap())
                        )));
                    }
                    if earlier.is_some_and(|earlier| earlier != table) {
                        return Err(StorageError::generic(format!(
                            "ID {id} is written as two record types"
                        )));
                    }
                }
            }
            claimed.insert(id, table);
        }
        Ok(())
    }

    /// `prepareDocumentActions` (`memory.ts:701-732`).
    fn prepare_document_actions(
        &self,
        writes: &[StorageWrite],
    ) -> Result<Vec<(i64, DocumentAction)>, StorageError> {
        let mut order: Vec<i64> = Vec::new();
        let mut actions: HashMap<i64, DocumentAction> = HashMap::new();
        for write in writes {
            match write {
                StorageWrite::DocumentCreate { record, content } => {
                    let action = action_entry(&mut order, &mut actions, record.id);
                    if action.create.is_some() || action.content.is_some() {
                        return Err(StorageError::generic(format!(
                            "Document {} has more than one content command",
                            record.id
                        )));
                    }
                    action.create = Some(record.clone());
                    action.content = Some(content.clone());
                }
                StorageWrite::DocumentChange { id, content } => {
                    let action = action_entry(&mut order, &mut actions, *id);
                    if action.content.is_some() {
                        return Err(StorageError::generic(format!(
                            "Document {id} has more than one content command"
                        )));
                    }
                    action.content = Some(content.clone());
                }
                StorageWrite::DocumentRetire { id } => {
                    let action = action_entry(&mut order, &mut actions, *id);
                    if action.retire {
                        return Err(StorageError::generic(format!(
                            "Document {id} is retired more than once"
                        )));
                    }
                    action.retire = true;
                }
                _ => {}
            }
        }
        Ok(order
            .into_iter()
            .map(|id| (id, actions.remove(&id).unwrap()))
            .collect())
    }

    /// `checkDocumentActions` (`memory.ts:734-761`).
    fn check_document_actions(
        &self,
        actions: &[(i64, DocumentAction)],
    ) -> Result<(), StorageError> {
        let state = self.read_state()?;
        let mut live_counts: HashMap<String, i64> = HashMap::new();
        for (id, action) in actions {
            let existing = state.documents.get(id);
            if action.create.is_none() && existing.is_none() {
                return Err(StorageError::generic(format!("Unknown document: {id}")));
            }
            if action.create.is_some() && existing.is_some() {
                return Err(StorageError::generic(format!(
                    "Document {id} already exists"
                )));
            }
            if existing.is_some_and(|stored| stored.record.retired_at.is_some()) {
                return Err(StorageError::generic(format!("Document {id} is retired")));
            }
            let previous = existing.and_then(|stored| stored.revisions.last());
            if let Some(DocumentContent::Delta { version, .. }) = &action.content {
                let Some(previous) = previous else {
                    return Err(StorageError::generic(format!(
                        "Document {id} delta has no base"
                    )));
                };
                // Every revision carries the incarnation's definition version,
                // deltas included; a delta continues the chain when it names
                // the same version (upstream `previous.version !==
                // action.content.version` over `revisions.at(-1)`).
                let previous_version = match &previous.content {
                    DocumentContent::Base {
                        version: previous_version,
                        ..
                    }
                    | DocumentContent::Delta {
                        version: previous_version,
                        ..
                    } => *previous_version,
                };
                if previous_version != *version {
                    return Err(StorageError::generic(format!(
                        "Document {id} version transition requires a base"
                    )));
                }
            }

            let key = match &action.create {
                Some(create) => address_key(&DocumentAddress {
                    kind: create.kind.clone(),
                    scope: create.scope,
                    key: create.key.clone(),
                }),
                None => record_address_key(&existing.unwrap().record),
            };
            let current_id = state
                .document_addresses
                .get(&key)
                .and_then(|index| index.current_id);
            let mut live = match live_counts.get(&key) {
                Some(live) => *live,
                None => match current_id {
                    Some(_) => 1,
                    None => 0,
                },
            };
            if action.retire && current_id == Some(*id) {
                live -= 1;
            }
            if action.create.is_some() && !action.retire {
                live += 1;
            }
            live_counts.insert(key, live);
        }

        for live in live_counts.values() {
            if *live > 1 {
                return Err(StorageError::generic(
                    "Document address already has a current incarnation",
                ));
            }
        }
        Ok(())
    }

    /// `applyDocumentActions` (`memory.ts:763-805`).
    fn apply_document_actions(
        &self,
        state: &mut State,
        actions: &[(i64, DocumentAction)],
        seq: i64,
    ) {
        let mut next_id = self
            .next_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for (id, action) in actions {
            let mut stored = state.documents.get(id).cloned();
            if let Some(create) = &action.create {
                let record =
                    DocumentRecord::from_create(create.clone(), seq, action.retire.then_some(seq));
                let content = action
                    .content
                    .clone()
                    .expect("document create carries its base content");
                let revision = DocumentRevision { content, seq };
                let fresh = StoredDocumentState {
                    record,
                    revisions: vec![revision],
                };
                state.record_types.insert(*id, TableName::Document);
                let key = record_address_key(&fresh.record);
                let scope = scope_key(&fresh.record.scope);
                state.documents.insert(*id, fresh);
                let address = state.document_addresses.entry(key).or_default();
                insert_sorted(&mut address.ids, *id);
                insert_map_id(&mut state.document_ids_by_scope, scope, *id);
                *next_id = (*next_id).max(id + 1);
                stored = state.documents.get(id).cloned();
            } else if let Some(content) = &action.content {
                let stored = stored
                    .as_mut()
                    .expect("document change targets an existing incarnation");
                let revision = DocumentRevision {
                    content: content.clone(),
                    seq,
                };
                if matches!(revision.content, DocumentContent::Base { .. })
                    && is_current_only(&stored.record)
                {
                    stored.revisions = vec![revision];
                } else {
                    stored.revisions.push(revision);
                }
                state.documents.insert(*id, stored.clone());
            }

            let mut stored = stored.expect("document action targets a stored incarnation");
            if action.retire && action.create.is_none() {
                stored.record.retired_at = Some(seq);
                state.documents.insert(*id, stored.clone());
            }
            if action.retire && is_current_only(&stored.record) {
                stored.revisions.clear();
                state.documents.insert(*id, stored.clone());
            }
            if action.create.is_some() || action.retire {
                let key = record_address_key(&stored.record);
                let address = state
                    .document_addresses
                    .get_mut(&key)
                    .expect("address index exists");
                if action.retire && address.current_id == Some(*id) {
                    address.current_id = None;
                }
                if action.create.is_some() && !action.retire {
                    address.current_id = Some(*id);
                }
            }
        }
    }
}

fn table_name(table: TableName) -> &'static str {
    match table {
        TableName::Conversation => "conversation",
        TableName::Entry => "entry",
        TableName::Task => "task",
        TableName::Submission => "submission",
        TableName::Document => "document",
    }
}

/// First-touch action slot with deterministic write order (`memory.ts`
/// `prepareDocumentActions` map discipline).
fn action_entry<'a>(
    order: &mut Vec<i64>,
    actions: &'a mut HashMap<i64, DocumentAction>,
    id: i64,
) -> &'a mut DocumentAction {
    actions.entry(id).or_insert_with(|| {
        order.push(id);
        DocumentAction::default()
    });
    actions.get_mut(&id).unwrap()
}

impl Storage for MemoryStorage {
    fn commit(&self, writes: &[StorageWrite], _context: &Context) -> Result<i64, StorageError> {
        let mut prepared = self.prepare_commit(writes, None)?;
        self.apply_prepared(&mut prepared)
    }

    fn mint_id(&self) -> Result<i64, StorageError> {
        self.assert_open()?;
        let mut next_id = self
            .next_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // `Number.isSafeInteger` bound: 2^53 - 1.
        if *next_id > 9_007_199_254_740_991 {
            return Err(StorageError::generic("ID space is exhausted"));
        }
        let id = *next_id;
        *next_id += 1;
        Ok(id)
    }

    fn conversation(
        &self,
        id: i64,
        _context: &Context,
    ) -> Result<Option<ConversationRecord>, StorageError> {
        let state = self.read_state()?;
        Ok(state.conversations.get(&id).cloned())
    }

    fn scan_conversations(
        &self,
        query: ConversationQuery,
        limit: usize,
        cursor: Option<&Cursor>,
        _context: &Context,
    ) -> Result<Page<ConversationRecord>, StorageError> {
        let state = self.read_state()?;
        let ids: Vec<i64> = if let Some(owner_task_id) = query.owner_task_id {
            state
                .conversation_ids_by_owner_task
                .get(&owner_task_id)
                .cloned()
                .unwrap_or_default()
        } else if let Some(owner_conversation_id) = query.owner_conversation_id {
            state
                .conversation_ids_by_owner_conversation
                .get(&owner_conversation_id)
                .cloned()
                .unwrap_or_default()
        } else {
            state.conversation_ids.clone()
        };
        let after = cursor_id(cursor)?;
        let start = after.map(|after| upper_bound(&ids, after)).unwrap_or(0);
        let mut values: Vec<ConversationRecord> = Vec::new();
        for index in ids.iter().skip(start) {
            let index = *index;
            if values.len() > limit {
                break;
            }
            let value = state.conversations.get(&index).unwrap();
            if let Some(owner_conversation_id) = query.owner_conversation_id {
                if value.owner.as_ref().map(|owner| owner.conversation_id)
                    != Some(owner_conversation_id)
                {
                    continue;
                }
            }
            values.push(value.clone());
        }
        Ok(page(&values, limit))
    }

    fn entry(
        &self,
        id: i64,
        _context: &Context,
    ) -> Result<Option<EntryWithCommitSeq>, StorageError> {
        let state = self.read_state()?;
        let Some(entry) = state.entries.get(&id) else {
            return Ok(None);
        };
        Ok(Some(EntryWithCommitSeq {
            entry: entry.clone(),
            commit_seq: state.entry_commit_seqs[&id],
        }))
    }

    fn entry_visible(
        &self,
        conversation_id: i64,
        id: i64,
        _context: &Context,
    ) -> Result<Option<EntryWithCommitSeq>, StorageError> {
        let mut found: Option<EntryRecord> = None;
        self.visit_visible_entries(conversation_id, id, Some(id), &mut |entry| {
            found = Some(entry.clone());
            false
        })?;
        match found {
            Some(entry) if entry.id == id => {
                let state = self.read_state()?;
                Ok(Some(EntryWithCommitSeq {
                    entry,
                    commit_seq: state.entry_commit_seqs[&id],
                }))
            }
            _ => Ok(None),
        }
    }

    fn find_latest_head_marker(
        &self,
        conversation_id: i64,
        at_or_before_entry_id: Option<i64>,
        _context: &Context,
    ) -> Result<Option<EntryRecord>, StorageError> {
        let state = self.read_state()?;
        if !state.conversations.contains_key(&conversation_id) {
            return Err(StorageError::generic(format!(
                "Unknown conversation: {conversation_id}"
            )));
        }
        let mut current_id = conversation_id;
        let mut upper_entry_id = at_or_before_entry_id;
        loop {
            let ids = state
                .head_entry_ids
                .get(&current_id)
                .cloned()
                .unwrap_or_default();
            let index = match upper_entry_id {
                Some(upper) => upper_bound(&ids, upper) as i64 - 1,
                None => ids.len() as i64 - 1,
            };
            if index >= 0 {
                let entry = state.entries.get(&ids[index as usize]).unwrap();
                return Ok(Some(entry.clone()));
            }
            let conversation = state.conversations.get(&current_id).unwrap();
            let Some(parent) = conversation.parent else {
                return Ok(None);
            };
            upper_entry_id = Some(match upper_entry_id {
                Some(upper) => upper.min(parent.at),
                None => parent.at,
            });
            current_id = parent.conversation_id;
        }
    }

    fn scan_entries(
        &self,
        query: EntryQuery,
        limit: usize,
        cursor: Option<&Cursor>,
        _context: &Context,
    ) -> Result<Page<EntryRecord>, StorageError> {
        let after = cursor_id(cursor)?;
        let max_entry_id = match (query.max_entry_id, after) {
            (Some(max), Some(after)) => Some(max.min(after - 1)),
            (max, Some(after)) => max.map_or(Some(after - 1), |_| Some(after - 1)),
            (max, None) => max,
        };
        let mut visible: Vec<EntryRecord> = Vec::new();
        self.visit_visible_entries(
            query.conversation_id,
            query.min_entry_id.unwrap_or(i64::MIN),
            max_entry_id,
            &mut |entry| {
                visible.push(entry.clone());
                visible.len() <= limit
            },
        )?;
        Ok(page(&visible, limit))
    }

    fn task(&self, id: TaskId, _context: &Context) -> Result<Option<TaskRecord>, StorageError> {
        let state = self.read_state()?;
        Ok(state.tasks.get(&id).cloned())
    }

    fn scan_tasks(
        &self,
        query: TaskQuery,
        limit: usize,
        cursor: Option<&Cursor>,
        _context: &Context,
    ) -> Result<Page<TaskRecord>, StorageError> {
        let state = self.read_state()?;
        let ids: Vec<i64> = match query.status {
            Some(status) => state.task_ids_by_status[task_status_slot(status)].clone(),
            None => state.task_ids.clone(),
        };
        let after = cursor_id(cursor)?;
        let start = after.map(|after| upper_bound(&ids, after)).unwrap_or(0);
        let mut values: Vec<TaskRecord> = Vec::new();
        for index in ids.iter().skip(start) {
            let index = *index;
            if values.len() > limit {
                break;
            }
            let value = state.tasks.get(&index).unwrap();
            if let Some(conversation_id) = query.conversation_id {
                if value.conversation_id != conversation_id {
                    continue;
                }
            }
            if let Some(kind) = &query.kind {
                if &value.kind != kind {
                    continue;
                }
            }
            if query
                .abort_requested
                .is_some_and(|abort| value.abort_requested != abort)
            {
                continue;
            }
            if query
                .background
                .is_some_and(|background| value.background != background)
            {
                continue;
            }
            values.push(value.clone());
        }
        Ok(page(&values, limit))
    }

    fn submission(
        &self,
        id: i64,
        _context: &Context,
    ) -> Result<Option<SubmissionRecord>, StorageError> {
        let state = self.read_state()?;
        Ok(state.submissions.get(&id).cloned())
    }

    fn scan_submissions(
        &self,
        query: SubmissionQuery,
        limit: usize,
        cursor: Option<&Cursor>,
        _context: &Context,
    ) -> Result<Page<SubmissionRecord>, StorageError> {
        let state = self.read_state()?;
        let ids: Vec<i64> = match query.status {
            Some(status) => state.submission_ids_by_status[submission_status_slot(status)].clone(),
            None => state.submission_ids.clone(),
        };
        let after = cursor_id(cursor)?;
        let start = after.map(|after| upper_bound(&ids, after)).unwrap_or(0);
        let mut values: Vec<SubmissionRecord> = Vec::new();
        for index in ids.iter().skip(start) {
            let index = *index;
            if values.len() > limit {
                break;
            }
            let value = state.submissions.get(&index).unwrap();
            if let Some(conversation_id) = query.conversation_id {
                if value.conversation_id != conversation_id {
                    continue;
                }
            }
            values.push(value.clone());
        }
        Ok(page(&values, limit))
    }

    fn submission_by_request(
        &self,
        conversation_id: i64,
        request_id: &str,
        _context: &Context,
    ) -> Result<Option<SubmissionRecord>, StorageError> {
        let state = self.read_state()?;
        let id = state
            .submission_ids_by_request
            .get(&conversation_id)
            .and_then(|requests| requests.get(request_id).copied());
        match id {
            Some(id) => Ok(state.submissions.get(&id).cloned()),
            None => Ok(None),
        }
    }

    fn find_document(
        &self,
        address: &DocumentAddress,
        at: DocumentPoint,
        _context: &Context,
    ) -> Result<Option<DocumentRecord>, StorageError> {
        let state = self.read_state()?;
        let index = state.document_addresses.get(&address_key(address));
        match at {
            DocumentPoint::Current => match index.and_then(|index| index.current_id) {
                Some(current_id) => Ok(Some(
                    state.documents.get(&current_id).unwrap().record.clone(),
                )),
                None => Ok(None),
            },
            DocumentPoint::Seq(at) => {
                for id in index.map(|index| index.ids.clone()).unwrap_or_default() {
                    let record = &state.documents.get(&id).unwrap().record;
                    if is_alive_at(record, DocumentPoint::Seq(at)) {
                        return Ok(Some(record.clone()));
                    }
                }
                Ok(None)
            }
        }
    }

    fn document(
        &self,
        id: i64,
        at: DocumentPoint,
        _context: &Context,
    ) -> Result<Option<StoredDocument>, StorageError> {
        self.materialize_document(id, at)
    }

    fn scan_documents(
        &self,
        query: DocumentQuery,
        limit: usize,
        cursor: Option<&Cursor>,
        _context: &Context,
    ) -> Result<Page<DocumentRecord>, StorageError> {
        let state = self.read_state()?;
        let ids = state
            .document_ids_by_scope
            .get(&scope_key(&query.scope))
            .cloned()
            .unwrap_or_default();
        let after = cursor_id(cursor)?;
        let start = after.map(|after| upper_bound(&ids, after)).unwrap_or(0);
        let mut values: Vec<DocumentRecord> = Vec::new();
        for index in ids.iter().skip(start) {
            let index = *index;
            if values.len() > limit {
                break;
            }
            let record = &state.documents.get(&index).unwrap().record;
            if let Some(kind) = &query.kind {
                if &record.kind != kind {
                    continue;
                }
            }
            if is_alive_at(record, query.at) {
                values.push(record.clone());
            }
        }
        Ok(page(&values, limit))
    }

    fn close(&self, _context: &Context) -> Result<(), StorageError> {
        self.closed.store(true, Ordering::SeqCst);
        Ok(())
    }
}

/// Rejected-batch constructor re-exported for callers that mirror upstream's
/// `throw new StorageRejected(...)` outside the backend.
pub fn storage_rejected(message: impl Into<String>) -> StorageError {
    let rejected = StorageRejected::new(message);
    StorageError {
        kind: StorageErrorKind::Rejected,
        message: rejected.message,
    }
}
