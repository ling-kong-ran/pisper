//! Port of `src/storage/sqlite/storage.ts`: the portable SQLite
//! implementation of the [`Storage`] contract.
//!
//! Divergences (structural, disclosed):
//! - **D3 (sync boundary), continuing.** The upstream methods are `async`
//!   over the synchronous facade; the port's [`Storage`] is synchronous
//!   outright.
//! - **D6 (mutation line), continuing.** `nextId`/`closed` sit behind one
//!   mutex for `&self` trait access; the connection serialization is the
//!   adapter's.
//! - **D4 (error channel), continuing.** The upstream plain `Error` throws
//!   map to [`StorageErrorKind::Generic`], `StorageRejected` to
//!   [`StorageErrorKind::Rejected`], with byte-equal messages.
//! - Row shapes and SQL texts are byte-pinned by the durable oracle
//!   (`sqlite_schema`, `sqlite_row_shapes`, and the conformance scenarios).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::agent_core::chord_support::context::Context;
use crate::chord::delta::{apply_immutable, op_from_json, op_to_json, Op};

use super::super::{EntryWithCommitSeq, Storage, StorageError, StorageErrorKind};
use super::database::{SqliteDatabase, SqliteRow, SqliteValue};
use super::migrations::apply_sqlite_migrations;
use crate::durable::types::{
    ConversationQuery, ConversationRecord, Cursor, DocumentAddress, DocumentContent,
    DocumentCopySource, DocumentCreate, DocumentHistory, DocumentPoint, DocumentQuery,
    DocumentRecord, DocumentScope, EntryQuery, EntryRecord, Page, StorageWrite, StoredDocument,
    SubmissionQuery, SubmissionRecord, SubmissionStatus, TaskId, TaskQuery, TaskRecord,
};

type StoredTask = TaskRecord;

/// Which table owns a global ID (`storage.ts` `TableName`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TableName {
    Conversation,
    Entry,
    Task,
    Submission,
    Document,
}

impl TableName {
    fn as_str(&self) -> &'static str {
        match self {
            TableName::Conversation => "conversation",
            TableName::Entry => "entry",
            TableName::Task => "task",
            TableName::Submission => "submission",
            TableName::Document => "document",
        }
    }

    fn from_record_type(value: &str) -> Option<TableName> {
        match value {
            "conversation" => Some(TableName::Conversation),
            "entry" => Some(TableName::Entry),
            "task" => Some(TableName::Task),
            "submission" => Some(TableName::Submission),
            "document" => Some(TableName::Document),
            _ => None,
        }
    }
}

/// One document mutation in a batch (`storage.ts` `DocumentAction`).
#[derive(Debug, Default, Clone)]
struct DocumentAction {
    create: Option<DocumentCreate>,
    copy: Option<DocumentCopySource>,
    content: Option<DocumentContent>,
    retire: bool,
}

/// The indexed identity columns of one document address
/// (`storage.ts` `addressParts` result).
struct AddressParts {
    kind: String,
    scope_kind: String,
    owner_id: i64,
    family: i64,
    key_value: String,
}

/// JSON encodes a string for indexed storage; some SQLite bindings replace
/// lone UTF-16 surrogates, so JSON encoding keeps indexed identities lossless
/// (`storage.ts` `encodeIndexedString`).
fn encode_indexed_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

fn parse_json<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, StorageError> {
    serde_json::from_str(text).map_err(|error| StorageError::generic(error.to_string()))
}

fn encode_json<T: serde::Serialize>(value: &T) -> Result<String, StorageError> {
    serde_json::to_string(value).map_err(|error| StorageError::generic(error.to_string()))
}

/// `cursorId(cursor)` (`storage.ts:72-77`).
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

/// `page(values, limit)` (`storage.ts:79-83`).
fn page<T>(values: Vec<T>, limit: usize, id_of: impl Fn(&T) -> i64) -> Page<T> {
    let has_next = values.len() > limit;
    let items: Vec<T> = values.into_iter().take(limit).collect();
    if !has_next {
        return Page { items, next: None };
    }
    let after = Value::from(id_of(&items[items.len() - 1]));
    let mut next = serde_json::Map::new();
    next.insert(String::from("after"), after);
    Page {
        items,
        next: Some(next),
    }
}

/// `scopeColumns(scope)` (`storage.ts:85-94`).
fn scope_columns(scope: &DocumentScope) -> (&'static str, i64) {
    match scope {
        DocumentScope::Session => ("session", 0),
        DocumentScope::Conversation { conversation_id } => ("conversation", *conversation_id),
        DocumentScope::Task { task_id } => ("task", *task_id),
    }
}

/// `addressParts(address)` (`storage.ts:96-104`).
fn address_parts(kind: &str, scope: &DocumentScope, key: Option<&str>) -> AddressParts {
    let (scope_kind, owner_id) = scope_columns(scope);
    AddressParts {
        kind: encode_indexed_string(kind),
        scope_kind: scope_kind.to_owned(),
        owner_id,
        family: if key.is_none() { 0 } else { 1 },
        key_value: encode_indexed_string(key.unwrap_or("")),
    }
}

fn address_parts_of_address(address: &DocumentAddress) -> AddressParts {
    address_parts(&address.kind, &address.scope, address.key.as_deref())
}

fn address_parts_of_create(create: &DocumentCreate) -> AddressParts {
    address_parts(&create.kind, &create.scope, create.key.as_deref())
}

fn address_parts_of_record(record: &DocumentRecord) -> AddressParts {
    address_parts(&record.kind, &record.scope, record.key.as_deref())
}

/// `isAliveAt(record, at)` (`storage.ts:111-114`).
fn is_alive_at(record: &DocumentRecord, at: DocumentPoint) -> bool {
    match at {
        DocumentPoint::Current => record.retired_at.is_none(),
        DocumentPoint::Seq(at) => {
            record.created_at <= at
                && (record.retired_at.is_none() || at < record.retired_at.unwrap())
        }
    }
}

/// `isCurrentOnly(record)` (`storage.ts:116-118`).
fn is_current_only(record: &DocumentRecord) -> bool {
    !matches!(record.scope, DocumentScope::Conversation { .. })
        || record.history == Some(DocumentHistory::Latest)
}

/// `writeId(write)` (`storage.ts:119-133`).
fn write_id(write: &StorageWrite) -> Option<i64> {
    match write {
        StorageWrite::Conversation { value } => Some(value.id),
        StorageWrite::Entry { value } => Some(value.id),
        StorageWrite::Task { value } => Some(value.id),
        StorageWrite::Submission { value } => Some(value.id),
        StorageWrite::DocumentCreate { record, .. } => Some(record.id),
        StorageWrite::DocumentCopy { record, .. } => Some(record.id),
        StorageWrite::DocumentChange { .. } | StorageWrite::DocumentRetire { .. } => None,
    }
}

fn submission_status_text(status: SubmissionStatus) -> String {
    match serde_json::to_value(status) {
        Ok(Value::String(text)) => text,
        _ => String::new(),
    }
}

/// Mutable identity state (`storage.ts` `SqliteStorage.nextId` / `closed`).
#[derive(Debug, Default)]
struct StorageState {
    next_id: i64,
    closed: bool,
}

/// Portable SQLite implementation of the Pico storage contract
/// (`storage.ts` `SqliteStorage`).
pub struct SqliteStorage {
    db: Arc<dyn SqliteDatabase>,
    state: Mutex<StorageState>,
}

impl SqliteStorage {
    /// Initialize storage over an owned SQLite database facade
    /// (`SqliteStorage.open`).
    pub fn open(db: Arc<dyn SqliteDatabase>) -> Result<Arc<SqliteStorage>, StorageError> {
        let result = Self::initialize(&db);
        if result.is_err() {
            let _ = db.close();
        }
        result
    }

    /// The `open` body over a borrowed facade.
    fn initialize(db: &Arc<dyn SqliteDatabase>) -> Result<Arc<SqliteStorage>, StorageError> {
        apply_sqlite_migrations(db.as_ref())?;
        let metadata = self_row(
            db,
            "SELECT next_id, next_seq FROM durable_metadata WHERE singleton = 1",
        )?;
        let metadata = metadata
            .and_then(|row| {
                row.text("next_id")
                    .and_then(|next_id| next_id.parse::<i64>().ok())
                    .zip(row.i64("next_seq"))
                    .map(|(next_id, _)| next_id)
            })
            .ok_or_else(|| StorageError::generic("Durable SQLite metadata is missing"))?;
        Ok(Arc::new(SqliteStorage {
            db: Arc::clone(db),
            state: Mutex::new(StorageState {
                next_id: metadata,
                closed: false,
            }),
        }))
    }

    // ─── Facade helpers ─────────────────────────────────────────────────

    fn select_row(
        &self,
        sql: &str,
        params: &[SqliteValue],
    ) -> Result<Option<SqliteRow>, StorageError> {
        self.db.select_row(sql, params)
    }

    fn select_all(
        &self,
        sql: &str,
        params: &[SqliteValue],
    ) -> Result<Vec<SqliteRow>, StorageError> {
        self.db.select_all(sql, params)
    }

    fn execute(&self, sql: &str, params: &[SqliteValue]) -> Result<(), StorageError> {
        self.db.run(sql, params)
    }

    fn state(&self) -> std::sync::MutexGuard<'_, StorageState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn assert_open(&self) -> Result<(), StorageError> {
        if self.state().closed {
            return Err(StorageError::generic("SqliteStorage is closed"));
        }
        Ok(())
    }

    /// `readConversation(id)` (`storage.ts:527-530`).
    fn read_conversation(&self, id: i64) -> Result<Option<ConversationRecord>, StorageError> {
        let row = self.select_row(
            "SELECT record FROM conversations WHERE id = ?",
            &[id.into()],
        )?;
        match row {
            None => Ok(None),
            Some(row) => Ok(Some(parse_json(row.text("record").unwrap_or_default())?)),
        }
    }

    /// `materializeDocument(id, at)` (`storage.ts:532-563`).
    fn materialize_document(
        &self,
        id: i64,
        at: DocumentPoint,
    ) -> Result<Option<StoredDocument>, StorageError> {
        let row = self.select_row("SELECT record FROM documents WHERE id = ?", &[id.into()])?;
        let Some(row) = row else {
            return Ok(None);
        };
        let record: DocumentRecord = parse_json(row.text("record").unwrap_or_default())?;
        if at != DocumentPoint::Current && is_current_only(&record) {
            return Err(StorageError::generic(format!(
                "Document {id} does not retain historical content"
            )));
        }
        if !is_alive_at(&record, at) {
            return Ok(None);
        }
        let upper = match at {
            DocumentPoint::Current => i64::MAX,
            DocumentPoint::Seq(at) => at,
        };
        let base = self.select_row(
            "SELECT seq, kind, version, content FROM document_revisions
				WHERE document_id = ? AND kind = 'base' AND seq <= ? ORDER BY seq DESC LIMIT 1",
            &[id.into(), upper.into()],
        )?;
        let Some(base) = base else {
            return Err(StorageError::generic(format!(
                "Document {id} is missing a required base"
            )));
        };
        let base_version = base.i64("version").unwrap_or_default();
        let base_seq = base.i64("seq").unwrap_or_default();
        let mut value: serde_json::Map<String, Value> =
            parse_json(base.text("content").unwrap_or_default())?;
        let tail = self.select_all(
            "SELECT seq, kind, version, content FROM document_revisions
				WHERE document_id = ? AND seq > ? AND seq <= ? ORDER BY seq",
            &[id.into(), base_seq.into(), upper.into()],
        )?;
        for revision in &tail {
            let kind = revision.text("kind").unwrap_or_default();
            let version = revision.i64("version").unwrap_or_default();
            if kind != "delta" || version != base_version {
                return Err(StorageError::generic(format!(
                    "Document {id} crosses a stored version boundary without a base"
                )));
            }
            let ops: Vec<Value> = parse_json(revision.text("content").unwrap_or_default())?;
            let ops: Vec<Op> = ops
                .iter()
                .map(op_from_json)
                .collect::<Result<Vec<Op>, _>>()
                .map_err(|error| StorageError::generic(error.message()))?;
            let applied = apply_immutable(Some(&Value::Object(value)), &ops)
                .map_err(|error| StorageError::generic(error.message()))?;
            value = applied
                .as_object()
                .cloned()
                .ok_or_else(|| StorageError::generic(error_message_document_apply(id)))?;
        }
        Ok(Some(StoredDocument {
            record,
            version: base_version,
            value,
            deltas_since_base: tail.len() as i64,
        }))
    }

    /// `candidateNextId(writes)` (`storage.ts:565-572`).
    fn candidate_next_id(&self, writes: &[StorageWrite]) -> i64 {
        let mut next_id = self.state().next_id;
        for write in writes {
            if let Some(id) = write_id(write) {
                next_id = next_id.max(id + 1);
            }
        }
        next_id
    }

    /// `checkGlobalIds(writes)` (`storage.ts:574-594`).
    fn check_global_ids(&self, writes: &[StorageWrite]) -> Result<(), StorageError> {
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
                    continue;
                }
            };
            let existing = self
                .select_row(
                    "SELECT record_type FROM record_ids WHERE id = ?",
                    &[id.into()],
                )?
                .and_then(|row| row.text("record_type").map(str::to_owned))
                .and_then(|text| TableName::from_record_type(&text));
            let earlier = claimed.get(&id).copied();
            match table {
                TableName::Conversation | TableName::Entry | TableName::Document => {
                    if let Some(existing) = existing {
                        return Err(StorageError::generic(format!(
                            "ID {id} already belongs to {}",
                            existing.as_str()
                        )));
                    }
                    if earlier.is_some() {
                        return Err(StorageError::generic(format!(
                            "ID {id} is written more than once"
                        )));
                    }
                }
                _ => {
                    if existing.is_some_and(|existing| existing != table) {
                        return Err(StorageError::generic(format!(
                            "ID {id} already belongs to {}",
                            existing.unwrap().as_str()
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

    /// `prepareDocumentActions(writes)` (`storage.ts:596-641`); insertion
    /// order preserved for the apply pass.
    fn prepare_document_actions(
        &self,
        writes: &[StorageWrite],
    ) -> Result<Vec<(i64, DocumentAction)>, StorageError> {
        let mut actions: Vec<(i64, DocumentAction)> = Vec::new();
        for write in writes {
            let id = match write {
                StorageWrite::DocumentCreate { record, .. } => record.id,
                StorageWrite::DocumentCopy { record, .. } => record.id,
                StorageWrite::DocumentChange { id, .. } => *id,
                StorageWrite::DocumentRetire { id } => *id,
                _ => continue,
            };
            let action = action_slot(&mut actions, id);
            match write {
                StorageWrite::DocumentCreate { record, content } => {
                    if action.create.is_some() || action.content.is_some() || action.copy.is_some()
                    {
                        return Err(StorageError::generic(format!(
                            "Document {id} has more than one content command"
                        )));
                    }
                    action.create = Some(record.clone());
                    action.content = Some(content.clone());
                }
                StorageWrite::DocumentCopy { record, source } => {
                    if action.create.is_some() || action.content.is_some() || action.copy.is_some()
                    {
                        return Err(StorageError::generic(format!(
                            "Document {id} has more than one content command"
                        )));
                    }
                    action.create = Some(record.clone());
                    action.copy = Some(*source);
                }
                StorageWrite::DocumentChange { content, .. } => {
                    if action.content.is_some() || action.copy.is_some() {
                        return Err(StorageError::generic(format!(
                            "Document {id} has more than one content command"
                        )));
                    }
                    action.content = Some(content.clone());
                }
                StorageWrite::DocumentRetire { .. } => {
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
        Ok(actions)
    }

    /// `checkDocumentActions(actions)` (`storage.ts:643-677`).
    fn check_document_actions(
        &self,
        actions: &[(i64, DocumentAction)],
    ) -> Result<(), StorageError> {
        let mut live_counts: HashMap<String, i64> = HashMap::new();
        for (id, action) in actions {
            if let Some(copy) = &action.copy {
                if actions.iter().any(|(existing, _)| existing == &copy.id) {
                    return Err(StorageError::rejected(format!(
                        "Document copy {id} source is changed in the copy batch"
                    )));
                }
            }
            let row =
                self.select_row("SELECT record FROM documents WHERE id = ?", &[(*id).into()])?;
            let existing: Option<DocumentRecord> = match row {
                None => None,
                Some(row) => Some(parse_json(row.text("record").unwrap_or_default())?),
            };
            if action.create.is_none() && existing.is_none() {
                return Err(StorageError::generic(format!("Unknown document: {id}")));
            }
            if action.create.is_some() && existing.is_some() {
                return Err(StorageError::generic(format!(
                    "Document {id} already exists"
                )));
            }
            if existing
                .as_ref()
                .is_some_and(|record| record.retired_at.is_some())
            {
                return Err(StorageError::generic(format!("Document {id} is retired")));
            }
            if let Some(DocumentContent::Delta { version, .. }) = &action.content {
                let previous = self.select_row(
                    "SELECT version FROM document_revisions WHERE document_id = ? ORDER BY seq DESC LIMIT 1",
                    &[(*id).into()],
                )?;
                let Some(previous) = previous else {
                    return Err(StorageError::generic(format!(
                        "Document {id} delta has no base"
                    )));
                };
                if previous.i64("version") != Some(*version) {
                    return Err(StorageError::generic(format!(
                        "Document {id} version transition requires a base"
                    )));
                }
            }
            // `action.create ?? existing!`
            let record = action
                .create
                .as_ref()
                .map(address_parts_of_create)
                .unwrap_or_else(|| {
                    address_parts_of_record(existing.as_ref().expect("checked above"))
                });
            let key = address_key_from_parts(&record);
            let mut live = match live_counts.get(&key) {
                Some(live) => *live,
                None => i64::from(self.current_document_id_of_parts(&record)?.is_some()),
            };
            if action.retire && existing.is_some() {
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

    /// `currentDocumentId(address)` over computed address parts
    /// (`storage.ts:679-692`).
    fn current_document_id_of_parts(
        &self,
        parts: &AddressParts,
    ) -> Result<Option<i64>, StorageError> {
        let row = self.select_row(
            "SELECT id FROM documents
				WHERE kind = ? AND scope_kind = ? AND owner_id = ? AND family = ? AND key_value = ? AND retired_at IS NULL
				LIMIT 1",
            &[
                SqliteValue::Text(parts.kind.clone()),
                SqliteValue::Text(parts.scope_kind.clone()),
                parts.owner_id.into(),
                parts.family.into(),
                SqliteValue::Text(parts.key_value.clone()),
            ],
        )?;
        Ok(row.and_then(|row| row.i64("id")))
    }

    /// `applyTableWrite(write, seq)` (`storage.ts:694-753`).
    fn apply_table_write(&self, write: &StorageWrite, seq: i64) -> Result<(), StorageError> {
        match write {
            StorageWrite::Conversation { value } => {
                self.claim_id(value.id, TableName::Conversation)?;
                self.execute(
                    "INSERT INTO conversations (id, owner_conversation_id, owner_task_id, record) VALUES (?, ?, ?, ?)",
                    &[
                        value.id.into(),
                        value
                            .owner
                            .as_ref()
                            .map(|owner| owner.conversation_id)
                            .map(SqliteValue::Integer)
                            .unwrap_or(SqliteValue::Null),
                        value
                            .owner
                            .as_ref()
                            .map(|owner| owner.task_id)
                            .map(SqliteValue::Integer)
                            .unwrap_or(SqliteValue::Null),
                        SqliteValue::Text(encode_json(value)?),
                    ],
                )
            }
            StorageWrite::Entry { value } => {
                self.claim_id(value.id, TableName::Entry)?;
                self.execute(
                    "INSERT INTO entries (id, conversation_id, head, commit_seq, record) VALUES (?, ?, ?, ?, ?)",
                    &[
                        value.id.into(),
                        value.conversation_id.into(),
                        value
                            .head
                            .map(SqliteValue::Integer)
                            .unwrap_or(SqliteValue::Null),
                        seq.into(),
                        SqliteValue::Text(encode_json(value)?),
                    ],
                )
            }
            StorageWrite::Task { value } => {
                self.claim_id(value.id, TableName::Task)?;
                self.execute(
                    "INSERT INTO tasks (id, conversation_id, kind, status, abort_requested, background, record)
						VALUES (?, ?, ?, ?, ?, ?, ?)
						ON CONFLICT(id) DO UPDATE SET conversation_id = excluded.conversation_id, kind = excluded.kind,
						status = excluded.status, abort_requested = excluded.abort_requested,
						background = excluded.background, record = excluded.record",
                    &[
                        value.id.into(),
                        value.conversation_id.into(),
                        SqliteValue::Text(encode_indexed_string(&value.kind)),
                        SqliteValue::Text(value.status().as_str().to_owned()),
                        SqliteValue::Integer(i64::from(value.abort_requested)),
                        SqliteValue::Integer(i64::from(value.background)),
                        SqliteValue::Text(encode_json(value)?),
                    ],
                )
            }
            StorageWrite::Submission { value } => {
                self.claim_id(value.id, TableName::Submission)?;
                self.execute(
                    "INSERT INTO submissions (id, conversation_id, request_id, status, record) VALUES (?, ?, ?, ?, ?)
						ON CONFLICT(id) DO UPDATE SET conversation_id = excluded.conversation_id,
						request_id = excluded.request_id, status = excluded.status, record = excluded.record",
                    &[
                        value.id.into(),
                        value.conversation_id.into(),
                        match &value.request_id {
                            Some(request_id) => {
                                SqliteValue::Text(encode_indexed_string(request_id))
                            }
                            None => SqliteValue::Null,
                        },
                        SqliteValue::Text(submission_status_text(value.status)),
                        SqliteValue::Text(encode_json(value)?),
                    ],
                )
            }
            StorageWrite::DocumentCreate { .. }
            | StorageWrite::DocumentCopy { .. }
            | StorageWrite::DocumentChange { .. }
            | StorageWrite::DocumentRetire { .. } => Ok(()),
        }
    }

    /// `claimId(id, table)` (`storage.ts:755-757`).
    fn claim_id(&self, id: i64, table: TableName) -> Result<(), StorageError> {
        self.execute(
            "INSERT OR IGNORE INTO record_ids (id, record_type) VALUES (?, ?)",
            &[id.into(), table.as_str().into()],
        )
    }

    /// `applyDocumentActions(actions, seq)` (`storage.ts:759-834`).
    fn apply_document_actions(
        &self,
        actions: &[(i64, DocumentAction)],
        seq: i64,
    ) -> Result<(), StorageError> {
        for (id, action) in actions {
            let mut content = action.content.clone();
            if let Some(copy) = &action.copy {
                let resolved: Result<DocumentContent, StorageError> = (|| {
                    let stored = self
                        .materialize_document(copy.id, copy.at)?
                        .ok_or_else(|| {
                            StorageError::generic(format!(
                                "Fork source document {} cannot be read",
                                copy.id
                            ))
                        })?;
                    let create = action
                        .create
                        .as_ref()
                        .expect("copy actions always carry the copied record");
                    let copied_conversation =
                        matches!(stored.record.scope, DocumentScope::Conversation { .. });
                    let create_conversation =
                        matches!(create.scope, DocumentScope::Conversation { .. });
                    if !copied_conversation
                        || !create_conversation
                        || stored.record.kind != create.kind
                        || stored.record.key != create.key
                        || stored.record.history != create.history
                        || stored.record.fork != create.fork
                    {
                        return Err(StorageError::generic(format!(
                            "Fork source document {} does not match the copied record",
                            copy.id
                        )));
                    }
                    Ok(DocumentContent::Base {
                        version: stored.version,
                        value: stored.value,
                    })
                })();
                content = match resolved {
                    Ok(content) => Some(content),
                    Err(error) => {
                        if error.kind == StorageErrorKind::Rejected {
                            return Err(error);
                        }
                        return Err(StorageError::rejected(format!(
                            "Document copy {id} was rejected"
                        )));
                    }
                };
            }
            let mut record: DocumentRecord;
            if let Some(create) = &action.create {
                record =
                    DocumentRecord::from_create(create.clone(), seq, action.retire.then_some(seq));
                let parts = address_parts_of_record(&record);
                self.claim_id(*id, TableName::Document)?;
                self.execute(
                    "INSERT INTO documents
						(id, kind, family, key_value, scope_kind, owner_id, created_at, retired_at, record)
						VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    &[
                        (*id).into(),
                        SqliteValue::Text(parts.kind.clone()),
                        parts.family.into(),
                        SqliteValue::Text(parts.key_value.clone()),
                        SqliteValue::Text(parts.scope_kind.clone()),
                        parts.owner_id.into(),
                        seq.into(),
                        if action.retire {
                            seq.into()
                        } else {
                            SqliteValue::Null
                        },
                        SqliteValue::Text(encode_json(&record)?),
                    ],
                )?;
            } else {
                record = parse_json(
                    self.select_row("SELECT record FROM documents WHERE id = ?", &[(*id).into()])?
                        .expect("checkDocumentActions verified the record exists")
                        .text("record")
                        .unwrap_or_default(),
                )?;
            }

            if let Some(content) = &content {
                if matches!(content, DocumentContent::Base { .. }) && is_current_only(&record) {
                    self.execute(
                        "DELETE FROM document_revisions WHERE document_id = ?",
                        &[(*id).into()],
                    )?;
                }
                let encoded_content = match content {
                    DocumentContent::Base { value, .. } => {
                        encode_json(&Value::Object(value.clone()))?
                    }
                    DocumentContent::Delta { ops, .. } => {
                        let wire: Vec<Value> = ops.iter().map(op_to_json).collect();
                        encode_json(&Value::Array(wire))?
                    }
                };
                self.execute(
                    "INSERT INTO document_revisions (document_id, seq, kind, version, content) VALUES (?, ?, ?, ?, ?)",
                    &[
                        (*id).into(),
                        seq.into(),
                        SqliteValue::Text(
                            match content {
                                DocumentContent::Base { .. } => "base",
                                DocumentContent::Delta { .. } => "delta",
                            }
                            .to_owned(),
                        ),
                        match content {
                            DocumentContent::Base { version, .. } => *version,
                            DocumentContent::Delta { version, .. } => *version,
                        }
                        .into(),
                        SqliteValue::Text(encoded_content),
                    ],
                )?;
            }

            if action.retire {
                if action.create.is_none() {
                    record.retired_at = Some(seq);
                    self.execute(
                        "UPDATE documents SET retired_at = ?, record = ? WHERE id = ?",
                        &[
                            seq.into(),
                            SqliteValue::Text(encode_json(&record)?),
                            (*id).into(),
                        ],
                    )?;
                }
                if is_current_only(&record) {
                    self.execute(
                        "DELETE FROM document_revisions WHERE document_id = ?",
                        &[(*id).into()],
                    )?;
                }
            }
        }
        Ok(())
    }
}

/// `error.message()` of a delta application that left a non-object document.
fn error_message_document_apply(id: i64) -> String {
    format!("Document {id} crossed a version boundary onto a non-object value")
}

/// `addressKey` over pre-computed parts.
fn address_key_from_parts(parts: &AddressParts) -> String {
    let array = Value::Array(vec![
        Value::from(parts.kind.clone()),
        Value::from(parts.scope_kind.clone()),
        Value::from(parts.owner_id),
        Value::from(parts.family),
        Value::from(parts.key_value.clone()),
    ]);
    serde_json::to_string(&array).unwrap_or_default()
}

/// The batch's mutable `DocumentAction` slot for one document ID, created on
/// first touch (upstream `Map.get ?? set`).
fn action_slot(actions: &mut Vec<(i64, DocumentAction)>, id: i64) -> &mut DocumentAction {
    let index = actions.iter().position(|(existing, _)| *existing == id);
    let index = index.unwrap_or_else(|| {
        actions.push((id, DocumentAction::default()));
        actions.len() - 1
    });
    &mut actions[index].1
}

/// The metadata row read shared by `open` and `commit`.
fn self_row(db: &Arc<dyn SqliteDatabase>, sql: &str) -> Result<Option<SqliteRow>, StorageError> {
    db.select_row(sql, &[])
}

impl Storage for SqliteStorage {
    fn commit(&self, writes: &[StorageWrite], _context: &Context) -> Result<i64, StorageError> {
        self.assert_open()?;
        let document_actions = self.prepare_document_actions(writes)?;
        let candidate_next_id = self.candidate_next_id(writes);
        let mut committed: Option<i64> = None;
        self.db.transaction(&mut || {
            let metadata = self
                .select_row(
                    "SELECT next_id, next_seq FROM durable_metadata WHERE singleton = 1",
                    &[],
                )?
                .ok_or_else(|| StorageError::generic("Durable SQLite metadata is missing"))?;
            let committed_seq = metadata
                .i64("next_seq")
                .ok_or_else(|| StorageError::generic("Durable SQLite metadata is missing"))?;
            self.check_global_ids(writes)?;
            self.check_document_actions(&document_actions)?;
            for write in writes {
                self.apply_table_write(write, committed_seq)?;
            }
            self.apply_document_actions(&document_actions, committed_seq)?;
            let stored_next_id = metadata
                .text("next_id")
                .and_then(|next_id| next_id.parse::<i64>().ok())
                .ok_or_else(|| StorageError::generic("Durable SQLite metadata is missing"))?;
            self.execute(
                "UPDATE durable_metadata SET next_id = ?, next_seq = ? WHERE singleton = 1",
                &[
                    SqliteValue::Text(stored_next_id.max(candidate_next_id).to_string()),
                    (committed_seq + 1).into(),
                ],
            )?;
            committed = Some(committed_seq);
            Ok(())
        })?;
        // One guard for the read-modify-write: evaluating the place and the
        // value as two `self.state()` temporaries would re-lock the same
        // non-re-entrant mutex from this thread and self-deadlock.
        {
            let mut state = self.state();
            state.next_id = state.next_id.max(candidate_next_id);
        }
        Ok(committed.expect("transaction body assigns the sequence"))
    }

    fn mint_id(&self) -> Result<i64, StorageError> {
        self.assert_open()?;
        let mut state = self.state();
        let next = state
            .next_id
            .checked_add(1)
            .ok_or_else(|| StorageError::generic("ID space is exhausted"))?;
        let id = state.next_id;
        state.next_id = next;
        Ok(id)
    }

    fn conversation(
        &self,
        id: i64,
        _context: &Context,
    ) -> Result<Option<ConversationRecord>, StorageError> {
        self.assert_open()?;
        self.read_conversation(id)
    }

    fn scan_conversations(
        &self,
        query: ConversationQuery,
        limit: usize,
        cursor: Option<&Cursor>,
        _context: &Context,
    ) -> Result<Page<ConversationRecord>, StorageError> {
        self.assert_open()?;
        let mut clauses: Vec<&str> = vec!["id > ?"];
        let mut params: Vec<SqliteValue> = vec![cursor_id(cursor)?.unwrap_or(-1).into()];
        if let Some(owner_conversation_id) = query.owner_conversation_id {
            clauses.push("owner_conversation_id = ?");
            params.push(owner_conversation_id.into());
        }
        if let Some(owner_task_id) = query.owner_task_id {
            clauses.push("owner_task_id = ?");
            params.push(owner_task_id.into());
        }
        params.push((limit as i64 + 1).into());
        let rows = self.select_all(
            &format!(
                "SELECT record FROM conversations WHERE {} ORDER BY id LIMIT ?",
                clauses.join(" AND ")
            ),
            &params,
        )?;
        let records: Vec<ConversationRecord> = rows
            .iter()
            .map(|row| parse_json(row.text("record").unwrap_or_default()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(page(records, limit, |record| record.id))
    }

    fn entry(
        &self,
        id: i64,
        _context: &Context,
    ) -> Result<Option<EntryWithCommitSeq>, StorageError> {
        self.assert_open()?;
        let row = self.select_row(
            "SELECT record, commit_seq FROM entries WHERE id = ?",
            &[id.into()],
        )?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(EntryWithCommitSeq {
            entry: parse_json(row.text("record").unwrap_or_default())?,
            commit_seq: row.i64("commit_seq").unwrap_or_default(),
        }))
    }

    fn entry_visible(
        &self,
        conversation_id: i64,
        id: i64,
        _context: &Context,
    ) -> Result<Option<EntryWithCommitSeq>, StorageError> {
        self.assert_open()?;
        let conversation = self.read_conversation(conversation_id)?;
        let Some(mut conversation) = conversation else {
            return Err(StorageError::generic(format!(
                "Unknown conversation: {conversation_id}"
            )));
        };
        let row = self.select_row(
            "SELECT record, commit_seq FROM entries WHERE id = ?",
            &[id.into()],
        )?;
        let Some(row) = row else {
            return Ok(None);
        };
        let entry: EntryRecord = parse_json(row.text("record").unwrap_or_default())?;
        let mut upper_entry_id = i64::MAX;
        while conversation.id != entry.conversation_id {
            let Some(parent) = conversation.parent else {
                return Ok(None);
            };
            upper_entry_id = upper_entry_id.min(parent.at);
            conversation = self
                .read_conversation(parent.conversation_id)?
                .ok_or_else(|| {
                    StorageError::generic(format!(
                        "Unknown conversation: {}",
                        parent.conversation_id
                    ))
                })?;
        }
        if entry.id > upper_entry_id {
            return Ok(None);
        }
        Ok(Some(EntryWithCommitSeq {
            entry,
            commit_seq: row.i64("commit_seq").unwrap_or_default(),
        }))
    }

    fn find_latest_head_marker(
        &self,
        conversation_id: i64,
        at_or_before_entry_id: Option<i64>,
        _context: &Context,
    ) -> Result<Option<EntryRecord>, StorageError> {
        self.assert_open()?;
        let mut conversation = self.read_conversation(conversation_id)?.ok_or_else(|| {
            StorageError::generic(format!("Unknown conversation: {conversation_id}"))
        })?;
        let mut upper: Option<i64> = at_or_before_entry_id;
        loop {
            let row = match upper {
                None => self.select_row(
                    "SELECT record FROM entries WHERE conversation_id = ? AND head IS NOT NULL ORDER BY id DESC LIMIT 1",
                    &[conversation.id.into()],
                )?,
                Some(upper) => self.select_row(
                    "SELECT record FROM entries WHERE conversation_id = ? AND head IS NOT NULL AND id <= ? ORDER BY id DESC LIMIT 1",
                    &[conversation.id.into(), upper.into()],
                )?,
            };
            if let Some(row) = row {
                return Ok(Some(parse_json(row.text("record").unwrap_or_default())?));
            }
            let Some(parent) = conversation.parent else {
                return Ok(None);
            };
            upper = Some(match upper {
                None => parent.at,
                Some(upper) => upper.min(parent.at),
            });
            conversation = self
                .read_conversation(parent.conversation_id)?
                .ok_or_else(|| {
                    StorageError::generic(format!(
                        "Unknown conversation: {}",
                        parent.conversation_id
                    ))
                })?;
        }
    }

    fn scan_entries(
        &self,
        query: EntryQuery,
        limit: usize,
        cursor: Option<&Cursor>,
        _context: &Context,
    ) -> Result<Page<EntryRecord>, StorageError> {
        self.assert_open()?;
        let mut conversation = self
            .read_conversation(query.conversation_id)?
            .ok_or_else(|| {
                StorageError::generic(format!("Unknown conversation: {}", query.conversation_id))
            })?;
        let after = cursor_id(cursor)?;
        let mut upper: Option<i64> = query.max_entry_id;
        if let Some(after) = after {
            upper = Some(upper.unwrap_or(i64::MAX).min(after - 1));
        }
        let mut values: Vec<EntryRecord> = Vec::new();
        loop {
            let mut clauses: Vec<&str> = vec!["conversation_id = ?"];
            let mut params: Vec<SqliteValue> = vec![conversation.id.into()];
            if let Some(min_entry_id) = query.min_entry_id {
                clauses.push("id >= ?");
                params.push(min_entry_id.into());
            }
            if let Some(upper) = upper {
                clauses.push("id <= ?");
                params.push(upper.into());
            }
            // SQLite treats a negative LIMIT as unbounded, matching the
            // upstream JS `limit + 1 - values.length` sign flip.
            params.push(((limit as i64) + 1 - values.len() as i64).into());
            let rows = self.select_all(
                &format!(
                    "SELECT record FROM entries WHERE {} ORDER BY id DESC LIMIT ?",
                    clauses.join(" AND ")
                ),
                &params,
            )?;
            values.extend(
                rows.iter()
                    .map(|row| parse_json::<EntryRecord>(row.text("record").unwrap_or_default()))
                    .collect::<Result<Vec<_>, _>>()?,
            );
            if values.len() > limit {
                break;
            }
            let Some(parent) = conversation.parent else {
                break;
            };
            upper = Some(match upper {
                None => parent.at,
                Some(upper) => upper.min(parent.at),
            });
            if query
                .min_entry_id
                .is_some_and(|min_entry_id| upper.is_some_and(|upper| upper < min_entry_id))
            {
                break;
            }
            conversation = self
                .read_conversation(parent.conversation_id)?
                .ok_or_else(|| {
                    StorageError::generic(format!(
                        "Unknown conversation: {}",
                        parent.conversation_id
                    ))
                })?;
        }
        Ok(page(values, limit, |entry| entry.id))
    }

    fn task(&self, id: TaskId, _context: &Context) -> Result<Option<StoredTask>, StorageError> {
        self.assert_open()?;
        let row = self.select_row("SELECT record FROM tasks WHERE id = ?", &[id.into()])?;
        match row {
            None => Ok(None),
            Some(row) => Ok(Some(parse_json(row.text("record").unwrap_or_default())?)),
        }
    }

    fn scan_tasks(
        &self,
        query: TaskQuery,
        limit: usize,
        cursor: Option<&Cursor>,
        _context: &Context,
    ) -> Result<Page<TaskRecord>, StorageError> {
        self.assert_open()?;
        let mut clauses: Vec<&str> = vec!["id > ?"];
        let mut params: Vec<SqliteValue> = vec![cursor_id(cursor)?.unwrap_or(-1).into()];
        if let Some(conversation_id) = query.conversation_id {
            clauses.push("conversation_id = ?");
            params.push(conversation_id.into());
        }
        if let Some(kind) = &query.kind {
            clauses.push("kind = ?");
            params.push(SqliteValue::Text(encode_indexed_string(kind)));
        }
        if let Some(status) = query.status {
            clauses.push("status = ?");
            params.push(SqliteValue::Text(status.as_str().to_owned()));
        }
        if let Some(abort_requested) = query.abort_requested {
            clauses.push("abort_requested = ?");
            params.push(i64::from(abort_requested).into());
        }
        if let Some(background) = query.background {
            clauses.push("background = ?");
            params.push(i64::from(background).into());
        }
        params.push((limit as i64 + 1).into());
        let rows = self.select_all(
            &format!(
                "SELECT record FROM tasks WHERE {} ORDER BY id LIMIT ?",
                clauses.join(" AND ")
            ),
            &params,
        )?;
        let records: Vec<TaskRecord> = rows
            .iter()
            .map(|row| parse_json(row.text("record").unwrap_or_default()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(page(records, limit, |record| record.id))
    }

    fn submission(
        &self,
        id: i64,
        _context: &Context,
    ) -> Result<Option<SubmissionRecord>, StorageError> {
        self.assert_open()?;
        let row = self.select_row("SELECT record FROM submissions WHERE id = ?", &[id.into()])?;
        match row {
            None => Ok(None),
            Some(row) => Ok(Some(parse_json(row.text("record").unwrap_or_default())?)),
        }
    }

    fn scan_submissions(
        &self,
        query: SubmissionQuery,
        limit: usize,
        cursor: Option<&Cursor>,
        _context: &Context,
    ) -> Result<Page<SubmissionRecord>, StorageError> {
        self.assert_open()?;
        let mut clauses: Vec<&str> = vec!["id > ?"];
        let mut params: Vec<SqliteValue> = vec![cursor_id(cursor)?.unwrap_or(-1).into()];
        if let Some(conversation_id) = query.conversation_id {
            clauses.push("conversation_id = ?");
            params.push(conversation_id.into());
        }
        if let Some(status) = query.status {
            clauses.push("status = ?");
            params.push(SqliteValue::Text(submission_status_text(status)));
        }
        params.push((limit as i64 + 1).into());
        let rows = self.select_all(
            &format!(
                "SELECT record FROM submissions WHERE {} ORDER BY id LIMIT ?",
                clauses.join(" AND ")
            ),
            &params,
        )?;
        let records: Vec<SubmissionRecord> = rows
            .iter()
            .map(|row| parse_json(row.text("record").unwrap_or_default()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(page(records, limit, |record| record.id))
    }

    fn submission_by_request(
        &self,
        conversation_id: i64,
        request_id: &str,
        _context: &Context,
    ) -> Result<Option<SubmissionRecord>, StorageError> {
        self.assert_open()?;
        let row = self.select_row(
            "SELECT record FROM submissions WHERE conversation_id = ? AND request_id = ?",
            &[
                conversation_id.into(),
                SqliteValue::Text(encode_indexed_string(request_id)),
            ],
        )?;
        match row {
            None => Ok(None),
            Some(row) => Ok(Some(parse_json(row.text("record").unwrap_or_default())?)),
        }
    }

    fn find_document(
        &self,
        address: &DocumentAddress,
        at: DocumentPoint,
        _context: &Context,
    ) -> Result<Option<DocumentRecord>, StorageError> {
        self.assert_open()?;
        let parts = address_parts_of_address(address);
        let row = match at {
            DocumentPoint::Current => self.select_row(
                "SELECT record FROM documents
					WHERE kind = ? AND scope_kind = ? AND owner_id = ? AND family = ? AND key_value = ?
					AND retired_at IS NULL ORDER BY created_at DESC LIMIT 1",
                &[
                    SqliteValue::Text(parts.kind.clone()),
                    SqliteValue::Text(parts.scope_kind.clone()),
                    parts.owner_id.into(),
                    parts.family.into(),
                    SqliteValue::Text(parts.key_value.clone()),
                ],
            )?,
            DocumentPoint::Seq(at) => self.select_row(
                "SELECT record FROM documents
					WHERE kind = ? AND scope_kind = ? AND owner_id = ? AND family = ? AND key_value = ?
					AND created_at <= ? AND (retired_at IS NULL OR retired_at > ?)
					ORDER BY created_at DESC LIMIT 1",
                &[
                    SqliteValue::Text(parts.kind.clone()),
                    SqliteValue::Text(parts.scope_kind.clone()),
                    parts.owner_id.into(),
                    parts.family.into(),
                    SqliteValue::Text(parts.key_value.clone()),
                    at.into(),
                    at.into(),
                ],
            )?,
        };
        match row {
            None => Ok(None),
            Some(row) => Ok(Some(parse_json(row.text("record").unwrap_or_default())?)),
        }
    }

    fn document(
        &self,
        id: i64,
        at: DocumentPoint,
        _context: &Context,
    ) -> Result<Option<StoredDocument>, StorageError> {
        self.assert_open()?;
        self.materialize_document(id, at)
    }

    fn scan_documents(
        &self,
        query: DocumentQuery,
        limit: usize,
        cursor: Option<&Cursor>,
        _context: &Context,
    ) -> Result<Page<DocumentRecord>, StorageError> {
        self.assert_open()?;
        let (scope_kind, owner_id) = scope_columns(&query.scope);
        let mut clauses: Vec<&str> = vec!["scope_kind = ?", "owner_id = ?", "id > ?"];
        let mut params: Vec<SqliteValue> = vec![
            SqliteValue::Text(scope_kind.to_owned()),
            owner_id.into(),
            cursor_id(cursor)?.unwrap_or(-1).into(),
        ];
        if let Some(kind) = &query.kind {
            clauses.push("kind = ?");
            params.push(SqliteValue::Text(encode_indexed_string(kind)));
        }
        match query.at {
            DocumentPoint::Current => clauses.push("retired_at IS NULL"),
            DocumentPoint::Seq(at) => {
                clauses.push("created_at <= ?");
                clauses.push("(retired_at IS NULL OR retired_at > ?)");
                params.push(at.into());
                params.push(at.into());
            }
        }
        params.push((limit as i64 + 1).into());
        let rows = self.select_all(
            &format!(
                "SELECT record FROM documents WHERE {} ORDER BY id LIMIT ?",
                clauses.join(" AND ")
            ),
            &params,
        )?;
        let records: Vec<DocumentRecord> = rows
            .iter()
            .map(|row| parse_json(row.text("record").unwrap_or_default()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(page(records, limit, |record| record.id))
    }

    fn close(&self, _context: &Context) -> Result<(), StorageError> {
        if self.state().closed {
            return Ok(());
        }
        self.state().closed = true;
        self.db.close()
    }
}
