//! The durable storage contract (`src/types.ts` `Storage`) and its backends.
//!
//! Divergences (disclosed):
//!
//! - **D3 (sync boundary).** Upstream every `Storage` method is `async`
//!   because all IO flows through the Node `ExecutionEnv` seam. The port's
//!   backends are in-process (memory) or `std::fs` (JSONL), so the trait is
//!   synchronous; commit ordering, atomicity, and every read/write semantic
//!   are unchanged, and callers still drive them from async contexts.
//! - **D4 (error channel).** Upstream throws `Error` subclasses; the port
//!   returns [`StorageError`], whose `Display` text matches the upstream
//!   `error.message` byte-for-byte.

use std::fmt;

use crate::agent_core::chord_support::context::Context;

pub mod jsonl;
pub mod memory;
pub mod sqlite;

pub use memory::{MemoryStorage, PreparedMemoryCommit};
pub use sqlite::{SqliteStorage, SQLITE_MIGRATIONS as SQLITE_STORAGE_MIGRATIONS};

use super::ids::Seq;
use super::types::{
    ConversationQuery, ConversationRecord, Cursor, DocumentAddress, DocumentPoint, DocumentQuery,
    DocumentRecord, EntryQuery, EntryRecord, Page, StorageWrite, StoredDocument, SubmissionQuery,
    SubmissionRecord, TaskId, TaskQuery, TaskRecord,
};

/// One storage entry plus the sequence of the commit that persisted it
/// (`types.ts` `Storage.entry` result).
#[derive(Debug, Clone, PartialEq)]
pub struct EntryWithCommitSeq {
    pub entry: EntryRecord,
    pub commit_seq: Seq,
}

/// Failure taxonomy across the storage surface (`errors.ts`, JSONL corruption
/// and poison errors, and the plain `Error` throws of the memory backend).
#[derive(Debug, Clone)]
pub struct StorageError {
    pub kind: StorageErrorKind,
    pub message: String,
}

/// `StorageError` classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageErrorKind {
    /// `StorageRejected`: the batch was rejected before any durable effect;
    /// the owning Session may continue safely.
    Rejected,
    /// `JsonlCorruptionError`: persisted bytes are unusable.
    Corruption,
    /// `JsonlStoragePoisonedError`: the backend is poisoned and must be
    /// reopened.
    Poisoned,
    /// The backend (or Session) is closed.
    Closed,
    /// Every other upstream `Error` throw.
    Generic,
}

impl StorageError {
    pub fn generic(message: impl Into<String>) -> Self {
        StorageError {
            kind: StorageErrorKind::Generic,
            message: message.into(),
        }
    }

    pub fn rejected(message: impl Into<String>) -> Self {
        StorageError {
            kind: StorageErrorKind::Rejected,
            message: message.into(),
        }
    }

    pub fn corruption(message: impl Into<String>) -> Self {
        StorageError {
            kind: StorageErrorKind::Corruption,
            message: message.into(),
        }
    }

    pub fn closed(message: impl Into<String>) -> Self {
        StorageError {
            kind: StorageErrorKind::Closed,
            message: message.into(),
        }
    }

    pub fn poisoned(cause: StorageError) -> Self {
        StorageError {
            kind: StorageErrorKind::Poisoned,
            message: cause.message,
        }
    }
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            StorageErrorKind::Poisoned => {
                f.write_str("JSONL storage is poisoned and must be reopened")
            }
            _ => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for StorageError {}

/// Atomic persistence boundary for Session records (`types.ts` `Storage`).
///
/// Storage trusts the owning Session to supply semantically valid records,
/// references, ancestry, and transitions. Implementations enforce atomicity,
/// global ID ownership, immutable conversation/entry creation, document record
/// consistency, and detached values; Session serializes commits.
pub trait Storage: Send + Sync {
    /// Atomically persist one batch and return its sequence. Once returned,
    /// later reads through this storage observe it.
    fn commit(&self, writes: &[StorageWrite], context: &Context) -> Result<Seq, StorageError>;

    /// Return a fresh candidate from the Session-global numeric ID namespace.
    fn mint_id(&self) -> Result<i64, StorageError>;

    /// Look up one conversation by exact ID.
    fn conversation(
        &self,
        id: i64,
        context: &Context,
    ) -> Result<Option<ConversationRecord>, StorageError>;

    /// Scan conversations in ascending ID order.
    fn scan_conversations(
        &self,
        query: ConversationQuery,
        limit: usize,
        cursor: Option<&Cursor>,
        context: &Context,
    ) -> Result<Page<ConversationRecord>, StorageError>;

    /// Look up one global entry and the sequence of the commit that persisted
    /// it.
    fn entry(&self, id: i64, context: &Context)
        -> Result<Option<EntryWithCommitSeq>, StorageError>;

    /// Look up one entry only when it is visible through the requested
    /// conversation's ancestry.
    fn entry_visible(
        &self,
        conversation_id: i64,
        id: i64,
        context: &Context,
    ) -> Result<Option<EntryWithCommitSeq>, StorageError>;

    /// Return the newest visible entry with a `head` at or below the optional
    /// inclusive cutoff. The returned entry is the marker; its `head` value is
    /// the range's actual lower bound.
    fn find_latest_head_marker(
        &self,
        conversation_id: i64,
        at_or_before_entry_id: Option<i64>,
        context: &Context,
    ) -> Result<Option<EntryRecord>, StorageError>;

    /// Scan the inclusive visible range newest-first, returning at most
    /// `limit` entries.
    fn scan_entries(
        &self,
        query: EntryQuery,
        limit: usize,
        cursor: Option<&Cursor>,
        context: &Context,
    ) -> Result<Page<EntryRecord>, StorageError>;

    /// Look up the latest complete record for one task.
    fn task(&self, id: TaskId, context: &Context) -> Result<Option<TaskRecord>, StorageError>;

    /// Scan task records matching every supplied filter.
    fn scan_tasks(
        &self,
        query: TaskQuery,
        limit: usize,
        cursor: Option<&Cursor>,
        context: &Context,
    ) -> Result<Page<TaskRecord>, StorageError>;

    /// Look up the latest complete record for one admitted submission.
    fn submission(
        &self,
        id: i64,
        context: &Context,
    ) -> Result<Option<SubmissionRecord>, StorageError>;

    /// Scan submissions matching every supplied filter in ascending ID order.
    fn scan_submissions(
        &self,
        query: SubmissionQuery,
        limit: usize,
        cursor: Option<&Cursor>,
        context: &Context,
    ) -> Result<Page<SubmissionRecord>, StorageError>;

    /// Find a submission by its conversation-scoped host deduplication key.
    fn submission_by_request(
        &self,
        conversation_id: i64,
        request_id: &str,
        context: &Context,
    ) -> Result<Option<SubmissionRecord>, StorageError>;

    /// Resolve the incarnation occupying one exact logical address at the
    /// selected point.
    fn find_document(
        &self,
        address: &DocumentAddress,
        at: DocumentPoint,
        context: &Context,
    ) -> Result<Option<DocumentRecord>, StorageError>;

    /// Materialize one specific incarnation by ID at the selected point
    /// without following a replacement at its address.
    fn document(
        &self,
        id: i64,
        at: DocumentPoint,
        context: &Context,
    ) -> Result<Option<StoredDocument>, StorageError>;

    /// Scan incarnations alive in one exact scope at the selected point.
    fn scan_documents(
        &self,
        query: DocumentQuery,
        limit: usize,
        cursor: Option<&Cursor>,
        context: &Context,
    ) -> Result<Page<DocumentRecord>, StorageError>;

    /// Release backend resources; all later operations must fail.
    fn close(&self, context: &Context) -> Result<(), StorageError>;
}
