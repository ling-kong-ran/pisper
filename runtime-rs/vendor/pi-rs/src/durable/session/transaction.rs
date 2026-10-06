//! Port of `src/session/transaction.ts`: the transaction for one Session
//! commit callback.
//!
//! Every operation is tracked upstream so callback settlement can reject
//! unfinished work; in the port all transaction operations are synchronous
//! (module divergence D5), so settlement only seals. Upstream hands each
//! acquisition the draft object (`change.state`) shared through the cached
//! `draftPromise`; the port hands each acquisition a [`DocumentDraft`]
//! forwarding to the one `Change` of the entry.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::agent_core::chord_support::context::Context;
use crate::chord::delta::{track, Change, Prepared, Tracker};

use super::super::documents::{
    address_id, check_record_scope, check_record_version, document_create,
    document_create_of_record, materialize_document_value, resolve_address, DocDefinition,
    ResolvedAddress,
};
use super::super::errors::{PlainError, ReadAfterWrite};
use super::super::ids::ROOT_CONVERSATION_ID;
use super::super::storage::Storage;
use super::super::types::{
    CheckpointInfo, ConversationOwner, ConversationOwnership, ConversationParent,
    ConversationQuery, ConversationRecord, Cursor, DocumentAddress, DocumentCommitChange,
    DocumentContent, DocumentCopySource, DocumentCreate, DocumentFork, DocumentPoint,
    DocumentQuery, DocumentRecord, DocumentScope, EntryDraft, EntryQuery, EntryRecord, Page,
    StorageWrite, SubmissionRecord, SubmissionSettlement, TaskOptions, TaskOwnership, TaskQuery,
    TaskRecord, TaskState, TaskStatus,
};
use super::forks::prepare_fork_document_copies;

/// A staged submission change: a settlement, or the placement of a queued
/// submission at its entry (`transaction.ts` `SubmissionChange`).
#[derive(Debug, Clone)]
pub enum SubmissionChange {
    Settle(SubmissionSettlement),
    Placed { entry: i64 },
}

/// Complete record after applying one change (`transaction.ts`
/// `applySubmissionChange`). Placement turns a queued input `placed` and a
/// queued write `done`; only a placed input can be answered. A settled record
/// stays.
pub fn apply_submission_change(
    current: &SubmissionRecord,
    change: &SubmissionChange,
) -> Result<SubmissionRecord, PlainError> {
    use super::super::types::{SubmissionStatus, SubmissionType};
    if current.status == SubmissionStatus::Done || current.status == SubmissionStatus::Unanswered {
        return Ok(current.clone());
    }
    match change {
        SubmissionChange::Placed { entry } => {
            if current.status != SubmissionStatus::Queued {
                return Err(PlainError::new(format!(
                    "Submission {} is not queued",
                    current.id
                )));
            }
            let mut next = current.clone();
            next.status = match current.r#type {
                SubmissionType::Input => SubmissionStatus::Placed,
                SubmissionType::Write => SubmissionStatus::Done,
            };
            next.entry = Some(*entry);
            Ok(next)
        }
        SubmissionChange::Settle(SubmissionSettlement::Done { answer }) => {
            if current.status != SubmissionStatus::Placed {
                return Err(PlainError::new(format!(
                    "Submission {} is not a placed input",
                    current.id
                )));
            }
            let mut next = current.clone();
            next.status = SubmissionStatus::Done;
            next.answer = Some(*answer);
            Ok(next)
        }
        SubmissionChange::Settle(SubmissionSettlement::Unanswered { reason, detail }) => {
            let mut next = current.clone();
            next.status = SubmissionStatus::Unanswered;
            next.reason = Some(reason.clone());
            next.detail = detail.clone();
            Ok(next)
        }
    }
}

const INTERNAL_SCAN_PAGE_SIZE: usize = 256;

/// Mutable per-document core shared between the Session cache and transactions
/// (`transaction.ts` `LoadedDocument` mutable members).
#[derive(Debug, Default, Clone)]
pub struct LoadedDocumentCore {
    /// Persisted definition version; older while the tracked value is
    /// migrated only in memory.
    pub stored_version: i64,
    /// Stored deltas after the newest base; advanced by adoption so the next
    /// predicate call needs no read.
    pub deltas_since_base: i64,
}

/// One committed document incarnation owned by the Session tracker cache
/// (`transaction.ts` `LoadedDocument`).
pub struct LoadedDocument {
    pub address_id: String,
    pub record: DocumentRecord,
    /// Definition version whose shape the tracked value has; access with
    /// another version reloads from Storage.
    pub value_version: i64,
    pub core: Mutex<LoadedDocumentCore>,
    pub tracker: Tracker,
}

impl LoadedDocument {
    pub fn new(
        address_id: String,
        record: DocumentRecord,
        stored_version: i64,
        value_version: i64,
        deltas_since_base: i64,
        tracker: Tracker,
    ) -> Arc<Self> {
        Arc::new(LoadedDocument {
            address_id,
            record,
            value_version,
            core: Mutex::new(LoadedDocumentCore {
                stored_version,
                deltas_since_base,
            }),
            tracker,
        })
    }

    /// The tracked committed value (`loaded.tracker.value`).
    pub fn value(&self) -> Value {
        self.tracker.value()
    }
}

/// Session services used by a transaction while it holds the mutation line
/// (`transaction.ts` `TransactionHost`).
pub trait TransactionHost: Send + Sync {
    fn storage(&self) -> &Arc<dyn Storage>;
    /// Return the cached current incarnation without loading.
    fn cached(&self, address_id: &str) -> Option<Arc<LoadedDocument>>;
    /// Return the cached current incarnation, cold-loading and migrating it
    /// when necessary.
    fn load(
        &self,
        definition: &DocDefinition,
        address_id: &str,
        address: &DocumentAddress,
        context: &Context,
    ) -> Result<Option<Arc<LoadedDocument>>, PlainError>;
    /// Install a newly committed incarnation.
    fn install(&self, document: Arc<LoadedDocument>);
    /// Remove a retired incarnation if it is still the cached occupant of its
    /// address.
    fn evict(&self, address_id: &str, record_id: i64);
    /// Stage writes that belong to every newly created or forked conversation,
    /// in its creating transaction.
    fn conversation_created(
        &self,
        tx: &Transaction,
        record: &ConversationRecord,
    ) -> Result<(), PlainError>;
}

/// The definition facets `Tx.createTask` consumes (`types.ts`
/// `TaskDefinition.name/version/initial`). The full typed task-definition
/// machinery arrives with the harness slice.
pub trait TaskDefinitionFacet {
    fn name(&self) -> &str;
    fn version(&self) -> i64;
    /// First durable checkpoint for a newly created task.
    fn initial(&self, input: &Value) -> Value;
}

/// Shared draft of one document acquisition (`change.state` upstream). Every
/// handle on one acquisition forwards to the same [`Change`]; settlement takes
/// the change out of the shared slot exactly once to prepare (or abort) it.
#[derive(Clone)]
pub struct DocumentDraft {
    inner: Arc<Mutex<Option<Change>>>,
}

impl DocumentDraft {
    fn new(change: Change) -> Self {
        DocumentDraft {
            inner: Arc::new(Mutex::new(Some(change))),
        }
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, Option<Change>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Take the open change for preparation or abortion (upstream
    /// `document.change.prepare()` / `document.change.abort()`).
    pub(crate) fn take_change(&self) -> Option<Change> {
        self.slot().take()
    }

    /// `change.read(path)`.
    pub fn read(
        &self,
        path: &[crate::chord::delta::Seg],
    ) -> Result<Option<Value>, crate::chord::delta::TrackerError> {
        let slot = self.slot();
        slot.as_ref().expect("draft change is open").read(path)
    }

    /// `change.set(path, value)`.
    pub fn set(
        &self,
        path: &[crate::chord::delta::Seg],
        value: Value,
    ) -> Result<(), crate::chord::delta::TrackerError> {
        let slot = self.slot();
        slot.as_ref()
            .expect("draft change is open")
            .set(path, value)
    }

    /// `change.delete(path)`.
    pub fn delete(
        &self,
        path: &[crate::chord::delta::Seg],
    ) -> Result<(), crate::chord::delta::TrackerError> {
        let slot = self.slot();
        slot.as_ref().expect("draft change is open").delete(path)
    }

    /// `change.push(path, items)`.
    pub fn push(
        &self,
        path: &[crate::chord::delta::Seg],
        items: Vec<Value>,
    ) -> Result<usize, crate::chord::delta::TrackerError> {
        let slot = self.slot();
        slot.as_ref()
            .expect("draft change is open")
            .push(path, items)
    }

    /// `change.pop(path)`.
    pub fn pop(
        &self,
        path: &[crate::chord::delta::Seg],
    ) -> Result<Option<Value>, crate::chord::delta::TrackerError> {
        let slot = self.slot();
        slot.as_ref().expect("draft change is open").pop(path)
    }

    // `change.splice(path, start, remove, items)`; the port returns the
    // retained tail like upstream.
    pub fn splice(
        &self,
        path: &[crate::chord::delta::Seg],
        start: isize,
        remove: isize,
        items: Vec<Value>,
    ) -> Result<Vec<Value>, crate::chord::delta::TrackerError> {
        let slot = self.slot();
        slot.as_ref()
            .expect("draft change is open")
            .splice(path, start, remove, items)
    }
}

/// Committed and candidate state for one task touched by this transaction
/// (`transaction.ts` `TransactionTask`).
#[derive(Default, Clone)]
struct TransactionTask {
    write: Option<TaskWrite>,
    publication_conversation_id: Option<i64>,
}

#[derive(Clone)]
struct TaskWrite {
    kind: TaskWriteKind,
    record: TaskRecord,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TaskWriteKind {
    Create,
    Replace,
}

/// Defaults a commit binds to (`transaction.ts` `TransactionScope`).
#[derive(Debug, Clone, Copy, Default)]
pub struct TransactionScope {
    /// Default `tx.createTask()` conversation.
    pub conversation_id: Option<i64>,
    /// Task whose runtime commit this is; stamped as `byTaskId` on appended
    /// entries.
    pub task_id: Option<i64>,
}

/// Storage/cache provenance of one staged document incarnation
/// (`transaction.ts` `DocumentTarget`).
enum DocumentTarget {
    Loaded {
        document: Arc<LoadedDocument>,
    },
    Created {
        record: DocumentCreate,
        version: i64,
        tracker: Tracker,
    },
    ForkCopy {
        record: DocumentCreate,
        source: DocumentCopySource,
    },
    RetireOnly {
        record: DocumentRecord,
    },
}

/// One document incarnation acquired, created, or retired by this transaction
/// (`transaction.ts` `DocumentEntry`).
struct DocumentEntry {
    address_id: String,
    address: DocumentAddress,
    definition: Mutex<Option<DocDefinition>>,
    state: Mutex<DocumentEntryState>,
}

#[derive(Default)]
struct DocumentEntryState {
    target: Option<DocumentTarget>,
    draft: Option<DocumentDraft>,
    prepared: Option<Prepared>,
    retire_on_commit: bool,
}

impl DocumentEntry {
    fn definition(&self) -> Option<DocDefinition> {
        self.definition
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn set_definition(&self, definition: &DocDefinition) {
        *self
            .definition
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(definition.clone());
    }
}

/// What one staged incarnation writes and publishes (`transaction.ts`
/// `DocumentPlan`).
struct DocumentPlan {
    record: PlanRecord,
    address_id: String,
    retire: bool,
    content: Option<PlanContent>,
    change: Option<PlanChange>,
    conversation_id: Option<i64>,
}

enum PlanRecord {
    Create(DocumentCreate),
    Committed(DocumentRecord),
}

impl PlanRecord {
    fn id(&self) -> i64 {
        match self {
            PlanRecord::Create(create) => create.id,
            PlanRecord::Committed(record) => record.id,
        }
    }

    fn scope(&self) -> DocumentScope {
        match self {
            PlanRecord::Create(create) => create.scope,
            PlanRecord::Committed(record) => record.scope,
        }
    }
}

#[derive(Clone)]
enum PlanContent {
    Create {
        record: DocumentCreate,
        content: DocumentContent,
    },
    Copy {
        record: DocumentCreate,
        source: DocumentCopySource,
    },
    Change {
        id: i64,
        content: DocumentContent,
    },
}

struct PlanChange {
    tracker: Tracker,
    prepared: Prepared,
    version: i64,
    loaded: Option<Arc<LoadedDocument>>,
    definition: Option<DocDefinition>,
}

/// Transaction for one Session commit callback (`transaction.ts`
/// `Transaction`).
pub struct Transaction {
    host: Arc<dyn TransactionHost>,
    context: Context,
    scope: TransactionScope,
    core: Mutex<TransactionCore>,
}

#[derive(Default)]
struct TransactionCore {
    sealed: bool,
    has_table_write: bool,
    writes: Vec<StorageWrite>,
    created_conversation_ids: Vec<i64>,
    fork_source_conversation_ids: Vec<i64>,
    fork_source_document_ids: Vec<i64>,
    tasks_by_id: HashMap<i64, TransactionTask>,
    submissions: HashMap<i64, SubmissionRecord>,
    submission_changes: Vec<(i64, SubmissionChange)>,
    plans: Vec<DocumentPlan>,
    documents: Vec<Arc<DocumentEntry>>,
    latest_document_by_address: HashMap<String, usize>,
}

impl Transaction {
    pub fn new(host: Arc<dyn TransactionHost>, context: Context, scope: TransactionScope) -> Self {
        Transaction {
            host,
            context,
            scope,
            core: Mutex::new(TransactionCore::default()),
        }
    }

    /// The transaction's bound commit context.
    pub fn context(&self) -> &Context {
        &self.context
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, TransactionCore> {
        self.core
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    // ─── Table reads ────────────────────────────────────────────────────

    fn read<T>(
        &self,
        method: &str,
        read: impl FnOnce() -> Result<T, PlainError>,
    ) -> Result<T, PlainError> {
        {
            let core = self.lock();
            if core.sealed {
                return Err(PlainError::new("Transaction has settled"));
            }
            if core.has_table_write {
                return Err(PlainError::new(ReadAfterWrite::new(method).to_string()));
            }
        }
        read()
    }

    fn write<T>(&self, write: impl FnOnce() -> Result<T, PlainError>) -> Result<T, PlainError> {
        {
            let mut core = self.lock();
            if core.sealed {
                return Err(PlainError::new("Transaction has settled"));
            }
            core.has_table_write = true;
        }
        write()
    }

    /// `tx.conversation(id)`.
    pub fn conversation(&self, id: i64) -> Result<Option<ConversationRecord>, PlainError> {
        self.read("conversation", || {
            self.host
                .storage()
                .conversation(id, &self.context)
                .map_err(|error| PlainError::new(error.to_string()))
        })
    }

    /// `tx.entry(id)`.
    pub fn entry(&self, id: i64) -> Result<Option<EntryRecord>, PlainError> {
        self.read("entry", || {
            Ok(self
                .host
                .storage()
                .entry(id, &self.context)
                .map_err(|error| PlainError::new(error.to_string()))?
                .map(|found| found.entry))
        })
    }

    /// `tx.task(id)`.
    pub fn task(&self, id: i64) -> Result<Option<TaskRecord>, PlainError> {
        self.read("task", || self.committed_task(id))
    }

    /// `tx.scanConversations(query, limit, cursor?)`.
    pub fn scan_conversations(
        &self,
        query: ConversationQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<ConversationRecord>, PlainError> {
        self.read("scanConversations", || {
            self.host
                .storage()
                .scan_conversations(query, limit, cursor, &self.context)
                .map_err(|error| PlainError::new(error.to_string()))
        })
    }

    /// `tx.scanEntries(query, limit, cursor?)`.
    pub fn scan_entries(
        &self,
        query: EntryQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<EntryRecord>, PlainError> {
        self.read("scanEntries", || {
            self.host
                .storage()
                .scan_entries(query, limit, cursor, &self.context)
                .map_err(|error| PlainError::new(error.to_string()))
        })
    }

    /// `tx.latestHeadMarker(conversationId)`.
    pub fn latest_head_marker(
        &self,
        conversation_id: i64,
    ) -> Result<Option<EntryRecord>, PlainError> {
        self.read("latestHeadMarker", || {
            self.host
                .storage()
                .find_latest_head_marker(conversation_id, None, &self.context)
                .map_err(|error| PlainError::new(error.to_string()))
        })
    }

    /// `tx.scanTasks(query, limit, cursor?)`.
    pub fn scan_tasks(
        &self,
        query: TaskQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<TaskRecord>, PlainError> {
        self.read("scanTasks", || {
            self.host
                .storage()
                .scan_tasks(query, limit, cursor, &self.context)
                .map_err(|error| PlainError::new(error.to_string()))
        })
    }

    /// Internal: committed submission record (`tx.submission`).
    pub fn submission(&self, id: i64) -> Result<Option<SubmissionRecord>, PlainError> {
        self.read("submission", || {
            self.host
                .storage()
                .submission(id, &self.context)
                .map_err(|error| PlainError::new(error.to_string()))
        })
    }

    /// Internal: committed submission with a conversation-scoped request ID.
    pub fn submission_by_request(
        &self,
        conversation_id: i64,
        request_id: &str,
    ) -> Result<Option<SubmissionRecord>, PlainError> {
        self.read("submissionByRequest", || {
            self.host
                .storage()
                .submission_by_request(conversation_id, request_id, &self.context)
                .map_err(|error| PlainError::new(error.to_string()))
        })
    }

    // ─── Table writes ───────────────────────────────────────────────────

    /// `tx.createConversation(options)`.
    pub fn create_conversation(
        &self,
        ownership: ConversationOwnership,
    ) -> Result<ConversationRecord, PlainError> {
        self.write(|| self.stage_conversation(None, ownership, None))
    }

    /// Internal final-form bootstrap path for the reserved root identity
    /// (`tx.createRootConversation`).
    pub fn create_root_conversation(&self) -> Result<ConversationRecord, PlainError> {
        self.write(|| {
            self.stage_conversation(
                None,
                ConversationOwnership::Ownerless,
                Some(ROOT_CONVERSATION_ID),
            )
        })
    }

    /// `tx.forkConversation(parentConversationId, at, options)`.
    pub fn fork_conversation(
        &self,
        parent_conversation_id: i64,
        at: i64,
        ownership: ConversationOwnership,
    ) -> Result<ConversationRecord, PlainError> {
        self.write(|| self.stage_conversation(Some((parent_conversation_id, at)), ownership, None))
    }

    /// `#stageConversation` (`transaction.ts:299-341`).
    fn stage_conversation(
        &self,
        parent: Option<(i64, i64)>,
        ownership: ConversationOwnership,
        reserved_id: Option<i64>,
    ) -> Result<ConversationRecord, PlainError> {
        let owner_task_id = match ownership {
            ConversationOwnership::Task { task_id } => Some(task_id),
            ConversationOwnership::Ownerless => None,
        };
        let id = match reserved_id {
            Some(reserved) => reserved,
            None => self
                .host
                .storage()
                .mint_id()
                .map_err(|error| PlainError::new(error.to_string()))?,
        };
        self.assert_open()?;
        let mut owner: Option<ConversationOwner> = None;
        if let Some(owner_task_id) = owner_task_id {
            let task = self.current_task(owner_task_id)?;
            self.assert_open()?;
            let task = task.ok_or_else(|| {
                PlainError::new(format!(
                    "Conversation owner task {owner_task_id} does not exist"
                ))
            })?;
            owner = Some(ConversationOwner {
                conversation_id: task.conversation_id,
                task_id: owner_task_id,
            });
        }
        let record = ConversationRecord {
            id,
            parent: parent.map(|(conversation_id, at)| ConversationParent {
                conversation_id,
                at,
            }),
            owner,
        };
        let copies = match parent {
            None => Vec::new(),
            Some((parent_conversation_id, at)) => prepare_fork_document_copies(
                self.host.storage(),
                parent_conversation_id,
                at,
                id,
                &self.context,
            )?,
        };
        self.assert_open()?;
        {
            let mut core = self.lock();
            for copy in copies {
                core.fork_source_document_ids.push(copy.source.id);
                let address = DocumentAddress {
                    kind: copy.record.kind.clone(),
                    scope: copy.record.scope,
                    key: copy.record.key.clone(),
                };
                let entry = Arc::new(DocumentEntry {
                    address_id: address_id(&address),
                    address,
                    definition: Mutex::new(None),
                    state: Mutex::new(DocumentEntryState {
                        target: Some(DocumentTarget::ForkCopy {
                            record: copy.record,
                            source: copy.source,
                        }),
                        ..Default::default()
                    }),
                });
                let index = core.documents.len();
                core.documents.push(Arc::clone(&entry));
                core.latest_document_by_address
                    .insert(entry.address_id.clone(), index);
            }
            if let Some((parent_conversation_id, _)) = parent {
                if !core
                    .fork_source_conversation_ids
                    .contains(&parent_conversation_id)
                {
                    core.fork_source_conversation_ids
                        .push(parent_conversation_id);
                }
            }
            core.created_conversation_ids.push(id);
            core.writes.push(StorageWrite::Conversation {
                value: record.clone(),
            });
        }
        self.host.conversation_created(self, &record)?;
        self.assert_open()?;
        Ok(record)
    }

    /// `tx.appendEntry(conversationId, value)`.
    pub fn append_entry(
        &self,
        conversation_id: i64,
        value: EntryDraft,
    ) -> Result<EntryRecord, PlainError> {
        self.write(|| {
            self.require_conversation(conversation_id)?;
            self.assert_open()?;
            let id = self
                .host
                .storage()
                .mint_id()
                .map_err(|error| PlainError::new(error.to_string()))?;
            self.assert_open()?;
            let record = EntryRecord::from_draft(value, id, conversation_id, self.scope.task_id);
            self.lock().writes.push(StorageWrite::Entry {
                value: record.clone(),
            });
            Ok(record)
        })
    }

    /// `tx.createTask(task, input, options)`.
    pub fn create_task(
        &self,
        definition: &dyn TaskDefinitionFacet,
        input: Value,
        options: TaskOptions,
    ) -> Result<i64, PlainError> {
        self.write(|| {
            let mut owner: Option<TaskRecord> = None;
            if let TaskOwnership::Task { task_id } = options.ownership {
                // Validated again against the owner's final candidate during
                // assembly.
                let candidate = self.current_task(task_id)?;
                self.assert_open()?;
                let candidate = candidate.ok_or_else(|| {
                    PlainError::new(format!("Task owner {task_id} does not exist"))
                })?;
                if options.background == Some(true) {
                    return Err(PlainError::new("A child task cannot be background"));
                }
                if options
                    .conversation_id
                    .is_some_and(|conversation_id| conversation_id != candidate.conversation_id)
                {
                    return Err(PlainError::new(format!(
                        "A child task lives in its owner's conversation {}",
                        candidate.conversation_id
                    )));
                }
                owner = Some(candidate);
            }
            let conversation_id = owner
                .as_ref()
                .map(|owner| owner.conversation_id)
                .or(options.conversation_id)
                .or(self.scope.conversation_id);
            let Some(conversation_id) = conversation_id else {
                return Err(PlainError::new(
                    "Tx.createTask() requires options.conversationId",
                ));
            };
            self.require_conversation(conversation_id)?;
            self.assert_open()?;
            let checkpoint = definition.initial(&input);
            let id = self
                .host
                .storage()
                .mint_id()
                .map_err(|error| PlainError::new(error.to_string()))?;
            self.assert_open()?;
            let record = TaskRecord {
                id,
                conversation_id,
                kind: definition.name().to_string(),
                version: definition.version(),
                input,
                owner: owner.as_ref().map(|owner| owner.id),
                background: options.background.unwrap_or(false),
                abort_requested: false,
                state: TaskState::Pending { checkpoint },
                memos: None,
            };
            self.lock().tasks_by_id.insert(
                id,
                TransactionTask {
                    write: Some(TaskWrite {
                        kind: TaskWriteKind::Create,
                        record,
                    }),
                    publication_conversation_id: None,
                },
            );
            Ok(id)
        })
    }

    /// Internal: create a submission record with a fresh ID. The caller builds
    /// the record with a `0` placeholder ID (upstream `{...create, id}`).
    pub fn create_submission(
        &self,
        mut record: SubmissionRecord,
    ) -> Result<SubmissionRecord, PlainError> {
        self.write(|| {
            self.require_conversation(record.conversation_id)?;
            self.assert_open()?;
            let id = self
                .host
                .storage()
                .mint_id()
                .map_err(|error| PlainError::new(error.to_string()))?;
            self.assert_open()?;
            record.id = id;
            self.lock().submissions.insert(id, record.clone());
            Ok(record)
        })
    }

    /// `tx.settleSubmission(id, settlement)`: resolved during assembly against
    /// the transaction's latest candidate record, falling back to committed
    /// state.
    pub fn settle_submission(
        &self,
        id: i64,
        settlement: SubmissionSettlement,
    ) -> Result<(), PlainError> {
        self.assert_open()?;
        let mut core = self.lock();
        core.has_table_write = true;
        core.submission_changes
            .push((id, SubmissionChange::Settle(settlement)));
        Ok(())
    }

    /// `tx.placeSubmission(id, entry)`: resolved during assembly like
    /// `settleSubmission()`.
    pub fn place_submission(&self, id: i64, entry: i64) -> Result<(), PlainError> {
        self.assert_open()?;
        let mut core = self.lock();
        core.has_table_write = true;
        core.submission_changes
            .push((id, SubmissionChange::Placed { entry }));
        Ok(())
    }

    /// Internal: replace one task record completely (`tx.setTask`). Tasks
    /// change their own state through their runtime.
    pub fn set_task(&self, value: TaskRecord) -> Result<(), PlainError> {
        self.assert_open()?;
        let mut core = self.lock();
        core.has_table_write = true;
        let task = core.tasks_by_id.entry(value.id).or_default();
        if let Some(write) = &task.write {
            if matches!(write.record.state, TaskState::Terminal { .. }) {
                return Err(PlainError::new(format!(
                    "Task {} already has a terminal candidate",
                    value.id
                )));
            }
            if write.record.conversation_id != value.conversation_id {
                return Err(PlainError::new(format!(
                    "Task {} cannot change conversations",
                    value.id
                )));
            }
        }
        let kind = match &task.write {
            Some(write) if write.kind == TaskWriteKind::Create => TaskWriteKind::Create,
            _ => TaskWriteKind::Replace,
        };
        task.write = Some(TaskWrite {
            kind,
            record: value,
        });
        Ok(())
    }

    /// Internal: candidate records of the tasks this transaction created or
    /// replaced so far (`tx.stagedTasks`).
    pub fn staged_tasks(&self) -> Vec<TaskRecord> {
        let core = self.lock();
        core.tasks_by_id
            .values()
            .filter_map(|task| task.write.as_ref().map(|write| write.record.clone()))
            .collect()
    }

    /// Internal: conversations this transaction created or forked so far
    /// (`tx.stagedConversations`).
    pub fn staged_conversations(&self) -> Vec<ConversationRecord> {
        let core = self.lock();
        core.writes
            .iter()
            .filter_map(|write| match write {
                StorageWrite::Conversation { value } => Some(value.clone()),
                _ => None,
            })
            .collect()
    }

    // ─── Documents ──────────────────────────────────────────────────────

    /// `tx.doc(token, ...args)` over the resolved address: acquire (or reuse)
    /// the draft of one document. `owner`/`key` follow [`resolve_address`];
    /// `seed` applies to families.
    pub fn doc(
        &self,
        definition: &DocDefinition,
        owner: Option<i64>,
        key: Option<&str>,
        seed: Option<Value>,
    ) -> Result<DocumentDraft, PlainError> {
        self.assert_open()?;
        let resolved = resolve_address(definition, owner, key)?;
        self.assert_task_documents_open(&resolved)?;
        let latest = self.latest_entry(&resolved.id);
        if let Some(latest) = &latest {
            let state = latest
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !state.retire_on_commit {
                if let Some(draft) = &state.draft {
                    return Ok(draft.clone());
                }
                if matches!(&state.target, Some(DocumentTarget::ForkCopy { .. })) {
                    drop(state);
                    return self.acquire_fork_copy(latest, definition);
                }
            }
        }
        let skip_load = match &latest {
            Some(latest) => {
                latest
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .retire_on_commit
            }
            None => false,
        };
        let seed = if definition.family { seed } else { None };
        let entry = Arc::new(DocumentEntry {
            address_id: resolved.id.clone(),
            address: resolved.address.clone(),
            definition: Mutex::new(Some(definition.clone())),
            state: Mutex::new(DocumentEntryState::default()),
        });
        self.push_entry(&entry);
        // Capture retirement before awaiting so a pending old acquisition and
        // its replacement stay distinct (upstream passes
        // `latest?.retireOnCommit === true`).
        self.acquire(&entry, seed.as_ref(), skip_load)?;
        let draft = entry
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .draft
            .clone()
            .expect("acquire installs the draft");
        Ok(draft)
    }

    /// `tx.retireDoc(token, ...args)`.
    pub fn retire_doc(
        &self,
        definition: &DocDefinition,
        owner: Option<i64>,
        key: Option<&str>,
    ) -> Result<(), PlainError> {
        self.assert_open()?;
        let resolved = resolve_address(definition, owner, key)?;
        let latest = self.latest_entry(&resolved.id);
        if let Some(latest) = &latest {
            let mut state = latest
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.retire_on_commit {
                return Ok(());
            }
            if let Some(DocumentTarget::ForkCopy { record, .. }) = &state.target {
                check_record_scope(definition, record)?;
                state.retire_on_commit = true;
                return Ok(());
            }
            if state.draft.is_some() {
                // Retirement of an acquired draft persists its final content
                // before retirement.
                state.retire_on_commit = true;
                return Ok(());
            }
        }
        let entry = Arc::new(DocumentEntry {
            address_id: resolved.id.clone(),
            address: resolved.address.clone(),
            definition: Mutex::new(Some(definition.clone())),
            state: Mutex::new(DocumentEntryState {
                retire_on_commit: true,
                ..Default::default()
            }),
        });
        self.push_entry(&entry);
        self.find_retirement(&entry)
    }

    fn latest_entry(&self, address_id: &str) -> Option<Arc<DocumentEntry>> {
        let core = self.lock();
        core.latest_document_by_address
            .get(address_id)
            .map(|index| Arc::clone(&core.documents[*index]))
    }

    fn push_entry(&self, entry: &Arc<DocumentEntry>) {
        let mut core = self.lock();
        let index = core.documents.len();
        core.documents.push(Arc::clone(entry));
        core.latest_document_by_address
            .insert(entry.address_id.clone(), index);
    }

    /// `#acquire` (`transaction.ts:585-620`).
    fn acquire(
        &self,
        entry: &Arc<DocumentEntry>,
        seed: Option<&Value>,
        skip_load: bool,
    ) -> Result<(), PlainError> {
        let definition = entry
            .definition()
            .expect("acquired entries carry their definition");
        let loaded = if skip_load {
            None
        } else {
            self.host.load(
                &definition,
                &entry.address_id,
                &entry.address,
                &self.context,
            )?
        };
        self.assert_open()?;
        if let Some(loaded) = loaded {
            let record = document_create_of_record(&loaded.record);
            check_record_scope(&definition, &record)?;
            let stored_version = loaded
                .core
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .stored_version;
            check_record_version(&definition, &record, stored_version)?;
            let mut state = entry
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.target = Some(DocumentTarget::Loaded {
                document: Arc::clone(&loaded),
            });
            state.draft = Some(DocumentDraft::new(loaded.tracker.begin_change()));
            return Ok(());
        }
        match &entry.address.scope {
            DocumentScope::Conversation { conversation_id } => {
                self.require_conversation(*conversation_id)?;
            }
            DocumentScope::Task { task_id } => {
                let task = self.current_task(*task_id)?;
                let task =
                    task.ok_or_else(|| PlainError::new(format!("Task {task_id} does not exist")))?;
                if task.status() == TaskStatus::Terminal {
                    return Err(PlainError::new(format!("Task {task_id} is terminal")));
                }
            }
            DocumentScope::Session => {}
        }
        self.assert_open()?;
        let value = (definition.initial)(seed);
        let id = self
            .host
            .storage()
            .mint_id()
            .map_err(|error| PlainError::new(error.to_string()))?;
        self.assert_open()?;
        let tracker = track(Value::Object(value));
        let record = document_create(&definition, &entry.address, id);
        let draft = DocumentDraft::new(tracker.begin_change());
        let mut state = entry
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.target = Some(DocumentTarget::Created {
            record,
            version: definition.version,
            tracker,
        });
        state.draft = Some(draft);
        Ok(())
    }

    /// `#acquireForkCopy` (`transaction.ts:622-647`).
    fn acquire_fork_copy(
        &self,
        entry: &Arc<DocumentEntry>,
        definition: &DocDefinition,
    ) -> Result<DocumentDraft, PlainError> {
        let (record, source) = {
            let state = entry
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match &state.target {
                Some(DocumentTarget::ForkCopy { record, source }) => (record.clone(), *source),
                _ => return Err(PlainError::new("Fork copy target missing")),
            }
        };
        let stored = self
            .host
            .storage()
            .document(source.id, source.at, &self.context)
            .map_err(|error| PlainError::new(error.to_string()))?;
        self.assert_open()?;
        let stored = stored.ok_or_else(|| {
            PlainError::new(format!("Fork source document {} cannot be read", source.id))
        })?;
        let source_record = document_create_of_record(&stored.record);
        if !matches!(stored.record.scope, DocumentScope::Conversation { .. })
            || source_record.kind != record.kind
            || source_record.key != record.key
            || source_record.history != record.history
            || source_record.fork != record.fork
        {
            return Err(PlainError::new(format!(
                "Fork source document {} does not match the copied record",
                source.id
            )));
        }
        let value = materialize_document_value(definition, &record, stored.version, &stored.value)?;
        entry.set_definition(definition);
        let tracker = track(Value::Object(value));
        let draft = DocumentDraft::new(tracker.begin_change());
        let mut state = entry
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.target = Some(DocumentTarget::Created {
            record,
            version: definition.version,
            tracker,
        });
        state.draft = Some(draft.clone());
        Ok(draft)
    }

    /// `#findRetirement` (`transaction.ts:649-657`).
    fn find_retirement(&self, entry: &Arc<DocumentEntry>) -> Result<(), PlainError> {
        let record = match self.host.cached(&entry.address_id) {
            Some(cached) => Some(cached.record.clone()),
            None => self
                .host
                .storage()
                .find_document(&entry.address, DocumentPoint::Current, &self.context)
                .map_err(|error| PlainError::new(error.to_string()))?,
        };
        self.assert_open()?;
        if let Some(record) = record {
            let definition = entry
                .definition()
                .expect("retirement entries carry their definition");
            check_record_scope(&definition, &document_create_of_record(&record))?;
            let mut state = entry
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.target = Some(DocumentTarget::RetireOnly { record });
        }
        Ok(())
    }

    // ─── Settlement ─────────────────────────────────────────────────────

    /// Seal after callback failure: abort every change (`settleFailure`).
    pub fn settle_failure(&self) {
        self.lock().sealed = true;
        self.abort_changes();
    }

    /// Seal after callback success, prepare every open change, and assemble
    /// the atomic batch (`settleSuccess`). Any failure aborts every change
    /// before Storage admission.
    pub fn settle_success(&self) -> Result<Vec<StorageWrite>, PlainError> {
        self.lock().sealed = true;
        let prepare_result = self.prepare_changes();
        if let Err(error) = prepare_result {
            self.abort_changes();
            return Err(error);
        }
        match self.assemble() {
            Ok(writes) => Ok(writes),
            Err(error) => {
                self.abort_changes();
                Err(error)
            }
        }
    }

    /// Synchronously prepare every open change; this revokes every draft
    /// (`settleSuccess`'s prepare loop).
    fn prepare_changes(&self) -> Result<(), PlainError> {
        let documents: Vec<Arc<DocumentEntry>> = self.lock().documents.clone();
        for entry in documents {
            let draft = {
                let mut state = entry
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.draft.take()
            };
            if let Some(draft) = draft {
                if let Some(change) = draft.take_change() {
                    let prepared = change
                        .prepare()
                        .map_err(|error| PlainError::new(error.message()))?;
                    entry
                        .state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .prepared = Some(prepared);
                }
            }
        }
        Ok(())
    }

    /// Abort every prepared change after Storage failure or when no write is
    /// required (`discard`).
    pub fn discard(&self) {
        self.abort_changes();
    }

    fn abort_changes(&self) {
        let documents: Vec<Arc<DocumentEntry>> = self.lock().documents.clone();
        for entry in documents {
            let draft = {
                let mut state = entry
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.draft.take()
            };
            if let Some(draft) = draft {
                if let Some(change) = draft.take_change() {
                    change.abort();
                }
            }
        }
    }

    /// Adopt every prepared change by pointer swap after Storage success and
    /// describe the publication (`adopt`).
    pub fn adopt(&self, seq: i64) -> Result<Vec<DocumentCommitChange>, PlainError> {
        let mut core = self.lock();
        let plans: Vec<DocumentPlan> = std::mem::take(&mut core.plans);
        drop(core);
        let mut publications: Vec<DocumentCommitChange> = Vec::new();
        for mut plan in plans {
            let committed = matches!(plan.record, PlanRecord::Committed(_));
            let mut record = match plan.record {
                PlanRecord::Committed(record) => record,
                PlanRecord::Create(create) => DocumentRecord::from_create(create, seq, None),
            };
            if plan.retire {
                record.retired_at = Some(seq);
            }
            let change = plan.change.take();
            if let Some(change) = change {
                let PlanChange {
                    tracker,
                    prepared,
                    version,
                    loaded,
                    definition,
                } = change;
                let _ = definition;
                // Snapshot the prepared publication before the abort/adopt
                // ownership move.
                let prepared_ops_empty = prepared.ops.is_empty();
                let prepared_ops = prepared.ops.clone();
                let prepared_object = prepared_value(&prepared);
                // A new incarnation is adopted unless it retires in the same
                // commit; a loaded one only when it changed.
                let adopt = match &loaded {
                    None => !plan.retire,
                    Some(_) => !prepared_ops_empty,
                };
                if adopt {
                    let _ = tracker.adopt(prepared.clone());
                } else {
                    prepared.abort();
                }
                if let Some(loaded) = &loaded {
                    let mut loaded_core = loaded
                        .core
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if loaded_core.stored_version < version {
                        loaded_core.stored_version = version;
                    }
                    if let Some(PlanContent::Change { content, .. }) = &plan.content {
                        match content {
                            DocumentContent::Base { .. } => loaded_core.deltas_since_base = 0,
                            DocumentContent::Delta { .. } => loaded_core.deltas_since_base += 1,
                        }
                    }
                } else if !plan.retire {
                    self.host.install(LoadedDocument::new(
                        plan.address_id.clone(),
                        record.clone(),
                        version,
                        version,
                        0,
                        tracker,
                    ));
                }
                let conversation_id = plan.conversation_id;
                if plan.retire {
                    if committed {
                        self.host.evict(&plan.address_id, record.id);
                    }
                    publications.push(DocumentCommitChange::Document {
                        record,
                        conversation_id,
                        version: None,
                        value: Value::Null,
                        ops: crate::durable::types::OpList(Vec::new()),
                    });
                } else if let Some(PlanContent::Copy { record: _, source }) = plan.content {
                    publications.push(DocumentCommitChange::DocumentCopy {
                        record,
                        conversation_id: conversation_id
                            .expect("copy plans resolve their conversation before adoption"),
                        source,
                    });
                } else if loaded.is_none() || !prepared_ops_empty {
                    let ops = if loaded.is_none() {
                        Vec::new()
                    } else {
                        prepared_ops
                    };
                    publications.push(DocumentCommitChange::Document {
                        record,
                        conversation_id,
                        version: Some(version),
                        value: Value::Object(prepared_object),
                        ops: crate::durable::types::OpList(ops),
                    });
                }
            } else {
                let conversation_id = plan.conversation_id;
                if plan.retire {
                    if committed {
                        self.host.evict(&plan.address_id, record.id);
                    }
                    publications.push(DocumentCommitChange::Document {
                        record,
                        conversation_id,
                        version: None,
                        value: Value::Null,
                        ops: crate::durable::types::OpList(Vec::new()),
                    });
                } else if let Some(PlanContent::Copy { record: _, source }) = plan.content {
                    publications.push(DocumentCommitChange::DocumentCopy {
                        record,
                        conversation_id: conversation_id
                            .expect("copy plans resolve their conversation before adoption"),
                        source,
                    });
                }
            }
        }
        Ok(publications)
    }

    /// `#assemble` (`transaction.ts:753-845`).
    fn assemble(&self) -> Result<Vec<StorageWrite>, PlainError> {
        let documents: Vec<Arc<DocumentEntry>> = self.lock().documents.clone();
        {
            let mut core = self.lock();
            for document in &documents {
                if let Some(plan) = plan_document(document)? {
                    core.plans.push(plan);
                }
            }
        }
        self.reject_fork_source_writes()?;
        self.validate_owners()?;
        {
            let core = self.lock();
            let replace_ids: Vec<i64> = core
                .tasks_by_id
                .iter()
                .filter(|(_, task)| {
                    task.write
                        .as_ref()
                        .is_some_and(|write| write.kind == TaskWriteKind::Replace)
                })
                .map(|(id, _)| *id)
                .collect();
            drop(core);
            for id in replace_ids {
                let candidate = {
                    let core = self.lock();
                    core.tasks_by_id
                        .get(&id)
                        .and_then(|task| task.write.as_ref().map(|write| write.record.clone()))
                };
                let Some(candidate) = candidate else { continue };
                let committed = self.committed_task(id)?;
                let Some(committed) = committed else {
                    return Err(PlainError::new(format!("Task {id} does not exist")));
                };
                if matches!(committed.state, TaskState::Terminal { .. }) {
                    return Err(PlainError::new(format!("Task {id} is already terminal")));
                }
                if committed.conversation_id != candidate.conversation_id {
                    return Err(PlainError::new(format!(
                        "Task {id} cannot change conversations"
                    )));
                }
            }
        }

        // Terminal settlement retires every task document, including ones
        // created by this transaction.
        let terminal_task_ids: Vec<i64> = {
            let core = self.lock();
            core.tasks_by_id
                .values()
                .filter_map(|task| task.write.as_ref())
                .filter(|write| matches!(write.record.state, TaskState::Terminal { .. }))
                .map(|write| write.record.id)
                .collect()
        };
        if !terminal_task_ids.is_empty() {
            let mut retiring: Vec<i64> = Vec::new();
            {
                let mut core = self.lock();
                for plan in core.plans.iter_mut() {
                    let DocumentScope::Task { task_id } = plan.record.scope() else {
                        continue;
                    };
                    if !terminal_task_ids.contains(&task_id) {
                        continue;
                    }
                    plan.retire = true;
                    retiring.push(plan.record.id());
                }
            }
            for task_id in &terminal_task_ids {
                let created_here = {
                    let core = self.lock();
                    core.tasks_by_id
                        .get(task_id)
                        .and_then(|task| task.write.as_ref())
                        .is_some_and(|write| write.kind == TaskWriteKind::Create)
                };
                if created_here {
                    continue;
                }
                let mut cursor: Option<Cursor> = None;
                loop {
                    let page = self
                        .host
                        .storage()
                        .scan_documents(
                            DocumentQuery {
                                scope: DocumentScope::Task { task_id: *task_id },
                                at: DocumentPoint::Current,
                                kind: None,
                            },
                            INTERNAL_SCAN_PAGE_SIZE,
                            cursor.as_ref(),
                            &self.context,
                        )
                        .map_err(|error| PlainError::new(error.to_string()))?;
                    for record in &page.items {
                        if retiring.contains(&record.id) {
                            continue;
                        }
                        retiring.push(record.id);
                        let mut core = self.lock();
                        core.plans.push(DocumentPlan {
                            address_id: address_id(&DocumentAddress {
                                kind: record.kind.clone(),
                                scope: record.scope,
                                key: record.key.clone(),
                            }),
                            record: PlanRecord::Committed(record.clone()),
                            retire: true,
                            content: None,
                            change: None,
                            conversation_id: None,
                        });
                    }
                    cursor = page.next;
                    if cursor.is_none() {
                        break;
                    }
                }
            }
        }

        // Resolve publication ownership before Storage admission so adoption
        // remains synchronous.
        {
            // Pre-resolve publication conversations for task-scoped plans that
            // publish (upstream awaits `#currentTask` inside the loop; the
            // port pre-resolves because its transaction is synchronous).
            let publishing_task_ids: Vec<i64> = {
                let core = self.lock();
                (0..core.plans.len())
                    .filter(|plan_index| {
                        let plan = &core.plans[*plan_index];
                        plan_publishes(plan)
                    })
                    .filter_map(|plan_index| match core.plans[plan_index].record.scope() {
                        DocumentScope::Task { task_id } => Some(task_id),
                        _ => None,
                    })
                    .collect()
            };
            for task_id in publishing_task_ids {
                let known = {
                    let mut core = self.lock();
                    let known = core
                        .tasks_by_id
                        .get(&task_id)
                        .and_then(|task| task.publication_conversation_id);
                    if known.is_none() {
                        core.tasks_by_id.entry(task_id).or_default();
                    }
                    known
                };
                if known.is_none() {
                    let current = self.current_task(task_id)?;
                    let mut core = self.lock();
                    core.tasks_by_id
                        .entry(task_id)
                        .or_default()
                        .publication_conversation_id = current.map(|task| task.conversation_id);
                }
            }
            let mut core = self.lock();
            for plan_index in 0..core.plans.len() {
                let plan_publishes = {
                    let plan = &core.plans[plan_index];
                    plan_publishes(plan)
                };
                if !plan_publishes {
                    continue;
                }
                let scope = core.plans[plan_index].record.scope();
                match scope {
                    DocumentScope::Conversation { conversation_id } => {
                        core.plans[plan_index].conversation_id = Some(conversation_id);
                    }
                    DocumentScope::Task { task_id } => {
                        core.plans[plan_index].conversation_id = core
                            .tasks_by_id
                            .get(&task_id)
                            .and_then(|task| task.publication_conversation_id);
                    }
                    DocumentScope::Session => {}
                }
            }
        }

        {
            let changes: Vec<(i64, SubmissionChange)> = self.lock().submission_changes.clone();
            for (id, change) in changes {
                let current = {
                    let core = self.lock();
                    core.submissions.get(&id).cloned()
                };
                let current = match current {
                    Some(current) => Some(current),
                    None => self
                        .host
                        .storage()
                        .submission(id, &self.context)
                        .map_err(|error| PlainError::new(error.to_string()))?,
                };
                let Some(current) = current else {
                    return Err(PlainError::new(format!("Submission {id} does not exist")));
                };
                let next = apply_submission_change(&current, &change)?;
                if next != current {
                    self.lock().submissions.insert(id, next);
                }
            }
        }

        let mut writes: Vec<StorageWrite> = self.lock().writes.clone();
        {
            let core = self.lock();
            for value in core.submissions.values() {
                writes.push(StorageWrite::Submission {
                    value: value.clone(),
                });
            }
            for task in core.tasks_by_id.values() {
                if let Some(write) = &task.write {
                    writes.push(StorageWrite::Task {
                        value: write.record.clone(),
                    });
                }
            }
        }
        {
            let mut core = self.lock();
            for plan_index in 0..core.plans.len() {
                // Checkpoint predicates run last, after every validation.
                let checkpointed = {
                    let plan = &core.plans[plan_index];
                    match (&plan.content, &plan.change) {
                        (
                            Some(PlanContent::Change {
                                content: DocumentContent::Delta { .. },
                                ..
                            }),
                            Some(change),
                        ) if change.loaded.is_some() => {
                            let loaded = change.loaded.as_ref().unwrap();
                            let info = CheckpointInfo {
                                deltas_since_base: loaded
                                    .core
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                                    .deltas_since_base,
                            };
                            let predicate = change
                                .definition
                                .as_ref()
                                .and_then(|definition| definition.checkpoint_when.clone());
                            match predicate {
                                Some(predicate) => predicate(
                                    &prepared_value(&change.prepared),
                                    &change.prepared.ops,
                                    info,
                                ),
                                None => false,
                            }
                        }
                        _ => false,
                    }
                };
                if checkpointed {
                    if let Some(PlanContent::Change { id, .. }) =
                        core.plans[plan_index].content.clone()
                    {
                        let change = core.plans[plan_index].change.as_ref().unwrap();
                        core.plans[plan_index].content = Some(PlanContent::Change {
                            id,
                            content: DocumentContent::Base {
                                version: change.version,
                                value: prepared_value(&change.prepared),
                            },
                        });
                    }
                }
                match core.plans[plan_index].content.clone() {
                    Some(PlanContent::Create { record, content }) => {
                        writes.push(StorageWrite::DocumentCreate { record, content });
                    }
                    Some(PlanContent::Copy { record, source }) => {
                        writes.push(StorageWrite::DocumentCopy { record, source });
                    }
                    Some(PlanContent::Change { id, content }) => {
                        writes.push(StorageWrite::DocumentChange { id, content });
                    }
                    None => {}
                }
                if core.plans[plan_index].retire {
                    writes.push(StorageWrite::DocumentRetire {
                        id: core.plans[plan_index].record.id(),
                    });
                }
            }
        }
        Ok(writes)
    }

    // ─── Helpers ────────────────────────────────────────────────────────

    /// `#validateOwners` (`transaction.ts:853-871`): new owned work needs a
    /// live owner, judged on the owner's final candidate.
    fn validate_owners(&self) -> Result<(), PlainError> {
        let mut owners: Vec<(&'static str, i64)> = Vec::new();
        {
            let core = self.lock();
            for write in &core.writes {
                if let StorageWrite::Conversation { value } = write {
                    if let Some(owner) = &value.owner {
                        owners.push(("Conversation owner task", owner.task_id));
                    }
                }
            }
            for task in core.tasks_by_id.values() {
                if let Some(write) = &task.write {
                    if write.kind == TaskWriteKind::Create {
                        if let Some(owner) = write.record.owner {
                            owners.push(("Task owner", owner));
                        }
                    }
                }
            }
        }
        for (what, task_id) in owners {
            let task = self.current_task(task_id)?;
            let Some(task) = task else {
                return Err(PlainError::new(format!("{what} {task_id} does not exist")));
            };
            match task.status() {
                TaskStatus::Terminal => {
                    return Err(PlainError::new(format!("{what} {task_id} is terminal")));
                }
                TaskStatus::Completing => {
                    return Err(PlainError::new(format!("{what} {task_id} is completing")));
                }
                _ => {}
            }
            if task.abort_requested {
                return Err(PlainError::new(format!("{what} {task_id} is abort-marked")));
            }
        }
        Ok(())
    }

    /// `#rejectForkSourceWrites` (`transaction.ts:873-892`).
    fn reject_fork_source_writes(&self) -> Result<(), PlainError> {
        let core = self.lock();
        for plan in &core.plans {
            if plan.content.is_none() && !plan.retire {
                continue;
            }
            if core.fork_source_document_ids.contains(&plan.record.id()) {
                return Err(PlainError::new(format!(
                    "Cannot change fork source document {} in the fork transaction",
                    plan.record.id()
                )));
            }
            if let DocumentScope::Conversation { conversation_id } = plan.record.scope() {
                let fork = match &plan.record {
                    PlanRecord::Create(create) => create.fork,
                    PlanRecord::Committed(record) => record.fork,
                };
                if fork == Some(DocumentFork::Current)
                    && core.fork_source_conversation_ids.contains(&conversation_id)
                {
                    return Err(PlainError::new(format!(
                        "Cannot fork conversation {conversation_id} while changing its current-policy documents"
                    )));
                }
            }
        }
        Ok(())
    }

    fn assert_open(&self) -> Result<(), PlainError> {
        if self.lock().sealed {
            return Err(PlainError::new("Transaction has settled"));
        }
        Ok(())
    }

    fn assert_task_documents_open(&self, resolved: &ResolvedAddress) -> Result<(), PlainError> {
        if let DocumentScope::Task { task_id } = resolved.address.scope {
            let core = self.lock();
            if let Some(task) = core.tasks_by_id.get(&task_id) {
                if let Some(write) = &task.write {
                    if matches!(write.record.state, TaskState::Terminal { .. }) {
                        return Err(PlainError::new(format!("Task {task_id} is terminal")));
                    }
                }
            }
        }
        Ok(())
    }

    /// `#requireConversation` (`transaction.ts:939-944`).
    fn require_conversation(&self, id: i64) -> Result<(), PlainError> {
        {
            let core = self.lock();
            if core.created_conversation_ids.contains(&id) {
                return Ok(());
            }
        }
        if self
            .host
            .storage()
            .conversation(id, &self.context)
            .map_err(|error| PlainError::new(error.to_string()))?
            .is_none()
        {
            return Err(PlainError::new(format!("Conversation {id} does not exist")));
        }
        Ok(())
    }

    /// `#currentTask` (`transaction.ts:956-958`): latest candidate task
    /// record, falling back to committed state; not a caller table read.
    fn current_task(&self, id: i64) -> Result<Option<TaskRecord>, PlainError> {
        let candidate = {
            let core = self.lock();
            core.tasks_by_id
                .get(&id)
                .and_then(|task| task.write.as_ref().map(|write| write.record.clone()))
        };
        match candidate {
            Some(candidate) => Ok(Some(candidate)),
            None => self.committed_task(id),
        }
    }

    /// `#committedTask` (`transaction.ts:960-964`).
    fn committed_task(&self, id: i64) -> Result<Option<TaskRecord>, PlainError> {
        self.host
            .storage()
            .task(id, &self.context)
            .map_err(|error| PlainError::new(error.to_string()))
    }
}

fn prepared_value(prepared: &Prepared) -> serde_json::Map<String, Value> {
    prepared.value.as_object().cloned().unwrap_or_default()
}

/// Whether adoption publishes the plan (`transaction.ts` `publishes`): every
/// creation, copy, and retirement, and a loaded incarnation that changed.
/// Whether adoption publishes the plan (v1.0.0): every creation, copy, and
/// retirement, and a loaded incarnation that writes content, which includes a
/// migration-only base so observers of the older shape receive the new value.
fn plan_publishes(plan: &DocumentPlan) -> bool {
    if plan.retire {
        return true;
    }
    match &plan.change {
        None => true,
        Some(change) => change.loaded.is_none() || plan.content.is_some(),
    }
}

/// Plan of one staged document (`transaction.ts` `planDocument`): its record,
/// content write, and prepared change. Retirement is decided later.
fn plan_document(document: &Arc<DocumentEntry>) -> Result<Option<DocumentPlan>, PlainError> {
    let state = document
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(target) = &state.target else {
        return Ok(None);
    };
    let plan_base = |record: PlanRecord| DocumentPlan {
        record,
        address_id: document.address_id.clone(),
        retire: state.retire_on_commit,
        content: None,
        change: None,
        conversation_id: None,
    };
    match target {
        DocumentTarget::Created {
            record,
            version,
            tracker,
        } => {
            let prepared = state
                .prepared
                .clone()
                .expect("created documents prepare before assembly");
            let mut plan = plan_base(PlanRecord::Create(record.clone()));
            plan.content = Some(PlanContent::Create {
                record: record.clone(),
                content: DocumentContent::Base {
                    version: *version,
                    value: prepared_value(&prepared),
                },
            });
            plan.change = Some(PlanChange {
                tracker: tracker.clone(),
                prepared,
                version: *version,
                loaded: None,
                definition: None,
            });
            Ok(Some(plan))
        }
        DocumentTarget::ForkCopy { record, source } => {
            let mut plan = plan_base(PlanRecord::Create(record.clone()));
            plan.content = Some(PlanContent::Copy {
                record: record.clone(),
                source: *source,
            });
            Ok(Some(plan))
        }
        DocumentTarget::RetireOnly { record } => {
            let plan = plan_base(PlanRecord::Committed(record.clone()));
            Ok(Some(plan))
        }
        DocumentTarget::Loaded { document: loaded } => {
            // The entry carries the definition; the loaded handle the tracker.
            let definition = document
                .definition()
                .expect("loaded documents carry their definition");
            let prepared = state
                .prepared
                .clone()
                .expect("loaded documents prepare before assembly");
            let version = definition.version;
            let id = loaded.record.id;
            let stored_version = loaded
                .core
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .stored_version;
            // A version change stores a base even without operations;
            // otherwise only a change stores a delta.
            let content = if stored_version < version {
                Some(PlanContent::Change {
                    id,
                    content: DocumentContent::Base {
                        version,
                        value: prepared_value(&prepared),
                    },
                })
            } else if !prepared.ops.is_empty() {
                Some(PlanContent::Change {
                    id,
                    content: DocumentContent::Delta {
                        version,
                        ops: prepared.ops.clone(),
                    },
                })
            } else {
                None
            };
            let mut plan = plan_base(PlanRecord::Committed(loaded.record.clone()));
            plan.content = content;
            plan.change = Some(PlanChange {
                tracker: loaded.tracker.clone(),
                prepared,
                version,
                loaded: Some(Arc::clone(loaded)),
                definition: Some(definition),
            });
            Ok(Some(plan))
        }
    }
}
