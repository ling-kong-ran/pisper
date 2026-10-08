//! Port of `packages/agent/src/harness/session/session.ts` (475 lines): the
//! session capability layer — [`MutationLine`], the typed session errors,
//! and [`StorageBackedSession`]/[`StorageBackedBranch`] over any
//! [`Storage`] (the concrete types the Task 5 [`BranchReader`]/[`SessionReader`]
//! projections and the repositories are built on).
//!
//! Disclosed substitutions:
//! - Upstream `MutationLine` chains promises; the port holds one
//!   `tokio::sync::Mutex` guard per mutation, which is the same
//!   serialization ("a nested public writer queues until its owning callback
//!   returns"; awaiting it inside the callback deadlocks — documented
//!   upstream, preserved here).
//! - Upstream error subclasses become structs implementing
//!   [`std::error::Error`] with the upstream messages; type checks in the
//!   oracles (`toBeInstanceOf`) map to `downcast_ref`.
//! - Branch objects are cheap per-call handles sharing one [`SessionCore`]
//!   (upstream caches one per name for object identity, which has no
//!   observable behavior the oracles pin).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;

use crate::agent_core::chord_support::Context;
use crate::agent_core::types::AgentMessage;
use crate::ai::types::primitives::StopReason;

use super::commit::insert_entry;
use super::types::{
    AscDescOrder, Branch as BranchTrait, BranchReader, BranchScan, BranchScanOrder, CommitResult,
    Entry, EntryQuery, EntryScan, IdGenerator, NewEntry, Session as SessionTrait, SessionMetadata,
    SessionMutation, SessionMutationReader, SessionMutator, SessionReader, SessionStats, Storage,
    StorageBranchScan, UuidV7Generator, Write, MAX_SAFE_INTEGER,
};
use super::values::{
    append_list as append_list_write, branch_tip, delete_list as delete_list_write,
    delete_value as delete_value_write, entry_label, session_name, set_value as set_value_write,
    ValueAddress,
};

/// Upstream lifecycle literals (`session.ts:232`).
const OPEN: u8 = 0;
const CLOSING: u8 = 1;
const CLOSED: u8 = 2;

const CLOSED_ERROR: &str = "Session is closed";

/// Upstream `SessionInvariantError` (`session.ts:44-50`): durable session
/// state is internally inconsistent and cannot be safely advanced.
#[derive(Debug)]
pub struct SessionInvariantError(pub String);

impl std::fmt::Display for SessionInvariantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for SessionInvariantError {}

/// Upstream `SessionInvalidBranchError` (`session.ts:52-63`).
#[derive(Debug)]
pub struct SessionInvalidBranchError {
    pub branch: String,
    pub reason: String,
}

impl std::fmt::Display for SessionInvalidBranchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Invalid branch {}: {}",
            serde_json::to_string(&self.branch).unwrap_or_default(),
            self.reason
        )
    }
}
impl std::error::Error for SessionInvalidBranchError {}

/// Upstream `SessionBranchExistsError` (`session.ts:65-74`).
#[derive(Debug)]
pub struct SessionBranchExistsError(pub String);

impl std::fmt::Display for SessionBranchExistsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Branch already exists: {}", self.0)
    }
}
impl std::error::Error for SessionBranchExistsError {}

/// Upstream `SessionPendingAssistantMessageError` (`session.ts:76-82`).
#[derive(Debug)]
pub struct SessionPendingAssistantMessageError;

impl std::fmt::Display for SessionPendingAssistantMessageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Cannot persist a pending assistant message")
    }
}
impl std::error::Error for SessionPendingAssistantMessageError {}

/// Upstream `SessionUnknownTargetError` (`session.ts:84-93`).
#[derive(Debug)]
pub struct SessionUnknownTargetError(pub String);

impl std::fmt::Display for SessionUnknownTargetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Unknown target: {}", self.0)
    }
}
impl std::error::Error for SessionUnknownTargetError {}

/// The sealed-error slot of the mutation line.
#[derive(Default)]
struct LineState {
    sealed_error: Option<String>,
}

/// Upstream `MutationLine` (`mutation-line.ts:1-23`): serializes complete
/// read-modify-write jobs for one Session and seals against late arrivals.
/// The line IS the guard held by the active mutation (see module docs).
#[derive(Clone, Default)]
pub struct MutationLine {
    state: Arc<tokio::sync::Mutex<LineState>>,
}

impl std::fmt::Debug for MutationLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MutationLine").finish_non_exhaustive()
    }
}

impl MutationLine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Upstream `seal(error)` (`mutation-line.ts:19-22`): record the first
    /// seal error and wait for the in-flight operation to finish.
    async fn seal(&self, error: &str) {
        let mut guard = self.state.lock().await;
        if guard.sealed_error.is_none() {
            guard.sealed_error = Some(error.to_string());
        }
    }
}

/// The shared session internals held by the session and its branch handles.
struct SessionCore {
    storage: Arc<dyn Storage>,
    mutation_line: MutationLine,
    id_generator: Arc<dyn IdGenerator>,
    state: AtomicU8,
    close_gate: tokio::sync::OnceCell<()>,
    on_close: Option<Box<dyn Fn() + Send + Sync>>,
}

impl SessionCore {
    fn assert_open(&self) -> anyhow::Result<()> {
        if self.state.load(Ordering::SeqCst) != OPEN {
            anyhow::bail!("{CLOSED_ERROR}");
        }
        Ok(())
    }
}

/// Upstream `StorageBackedSessionOptions` (`session.ts:38-42`).
#[derive(Default)]
pub struct StorageBackedSessionOptions {
    pub mutation_line: Option<MutationLine>,
    pub id_generator: Option<Arc<dyn IdGenerator>>,
    pub on_close: Option<Box<dyn Fn() + Send + Sync>>,
}

/// Upstream `StorageBackedSession` (`session.ts:224-475`): the
/// package-internal typed boundary shared by concrete session repositories.
pub struct StorageBackedSession {
    metadata: SessionMetadata,
    core: Arc<SessionCore>,
}

impl std::fmt::Debug for StorageBackedSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageBackedSession")
            .field("metadata", &self.metadata)
            .finish_non_exhaustive()
    }
}

/// Upstream `appendToBranch` entry union (`session.ts:424`). The Message
/// variant is the largest payload; boxing keeps the enum small and the match
/// arms unchanged.
pub enum AppendEntry {
    Message {
        message: Box<AgentMessage>,
    },
    Custom {
        custom_type: String,
        data: Option<serde_json::Value>,
    },
}

/// Upstream's pending-assistant guard (`session.ts:110-120`,
/// `session.ts:427-430`).
fn assert_not_pending_assistant(message: &AgentMessage) -> anyhow::Result<()> {
    if let AgentMessage::Assistant(assistant) = message {
        if assistant.stop_reason == StopReason::Pending {
            return Err(anyhow::Error::new(SessionPendingAssistantMessageError));
        }
    }
    Ok(())
}

/// The mutation admission: acquire the line, reject when sealed. The guard
/// rides on the returned mutation until `end` releases it.
async fn begin_session_mutation(core: &SessionCore) -> anyhow::Result<Box<dyn SessionMutation>> {
    core.assert_open()?;
    let guard = Arc::clone(&core.mutation_line.state).lock_owned().await;
    if let Some(error) = guard.sealed_error.clone() {
        anyhow::bail!("{error}");
    }
    Ok(Box::new(StorageBackedSessionMutation {
        storage: Arc::clone(&core.storage),
        line_guard: Mutex::new(Some(guard)),
        active: AtomicBool::new(true),
        commit_taken: AtomicBool::new(false),
        commit_result: tokio::sync::Mutex::new(None),
    }) as Box<dyn SessionMutation>)
}

/// The `Session::mutate` composition: one line acquisition around the
/// callback, ending the mutation even when the callback fails.
async fn run_mutation<T, F>(core: &SessionCore, mutation: F, context: Context) -> anyhow::Result<T>
where
    F: FnOnce(&dyn SessionMutator, Context) -> BoxFuture<'_, anyhow::Result<T>>,
{
    let handle = begin_session_mutation(core).await?;
    let outcome = mutation(handle.as_ref(), context.clone()).await;
    handle.end(context).await?;
    outcome
}

/// Upstream `getBranchTip(name, context)` (`session.ts:416-420`) over a
/// session core.
async fn get_branch_tip(
    core: &SessionCore,
    name: &str,
    context: Context,
) -> anyhow::Result<Option<String>> {
    let stored = core
        .storage
        .get_value(&branch_tip(name), context)
        .await?
        .ok_or_else(|| {
            anyhow::Error::new(SessionInvariantError(format!("Unknown branch: {name}")))
        })?;
    Ok(stored.value.as_str().map(str::to_string))
}

/// Upstream `appendToBranch(name, entry, context)` (`session.ts:422-454`)
/// over a session core.
async fn append_to_branch(
    core: &SessionCore,
    name: &str,
    entry: AppendEntry,
    context: Context,
) -> anyhow::Result<String> {
    core.assert_open()?;
    if let AppendEntry::Message { message } = &entry {
        assert_not_pending_assistant(message)?;
    }
    let id = core.id_generator.next(None);
    let id_result = id.clone();
    let name = name.to_string();
    run_mutation(
        core,
        |mutator, context| {
            Box::pin(async move {
                let tip = mutator
                    .get_value(&branch_tip(&name), context.clone())
                    .await?
                    .ok_or_else(|| {
                        anyhow::Error::new(SessionInvariantError(format!("Unknown branch: {name}")))
                    })?;
                let parent_id = tip.value.as_str().map(str::to_string);
                let staged = match entry {
                    AppendEntry::Message { message } => NewEntry::Message {
                        id: id.clone(),
                        parent_id,
                        message: *message,
                        terminate: None,
                    },
                    AppendEntry::Custom { custom_type, data } => NewEntry::Custom {
                        id: id.clone(),
                        parent_id,
                        custom_type,
                        data,
                    },
                };
                mutator
                    .commit(
                        vec![
                            insert_entry(staged),
                            set_value_write(
                                &branch_tip(&name),
                                serde_json::Value::String(id.clone()),
                            ),
                        ],
                        context.clone(),
                    )
                    .await?;
                Ok(())
            })
        },
        context,
    )
    .await?;
    Ok(id_result)
}

impl StorageBackedSession {
    /// Upstream `new StorageBackedSession(metadata, storage, options)`
    /// (`session.ts:235-241`).
    pub fn new(metadata: SessionMetadata, storage: Arc<dyn Storage>) -> Self {
        Self::with_options(metadata, storage, StorageBackedSessionOptions::default())
    }

    pub fn with_options(
        metadata: SessionMetadata,
        storage: Arc<dyn Storage>,
        options: StorageBackedSessionOptions,
    ) -> Self {
        StorageBackedSession {
            metadata,
            core: Arc::new(SessionCore {
                storage,
                mutation_line: options.mutation_line.unwrap_or_default(),
                id_generator: options
                    .id_generator
                    .unwrap_or_else(|| Arc::new(UuidV7Generator)),
                state: AtomicU8::new(OPEN),
                close_gate: tokio::sync::OnceCell::new(),
                on_close: options.on_close,
            }),
        }
    }

    fn assert_open(&self) -> anyhow::Result<()> {
        self.core.assert_open()
    }

    fn assert_valid_branch_name(name: &str) -> anyhow::Result<()> {
        if name.is_empty() {
            return Err(anyhow::Error::new(SessionInvalidBranchError {
                branch: name.to_string(),
                reason: "branch name must not be empty".to_string(),
            }));
        }
        if name.contains('\0') {
            return Err(anyhow::Error::new(SessionInvalidBranchError {
                branch: name.to_string(),
                reason: "branch name must not contain \\u0000".to_string(),
            }));
        }
        Ok(())
    }

    /// Upstream `getBranchTip(name, context)` (`session.ts:416-420`).
    pub async fn get_branch_tip(
        &self,
        name: &str,
        context: Context,
    ) -> anyhow::Result<Option<String>> {
        get_branch_tip(&self.core, name, context).await
    }

    /// Upstream `appendToBranch(name, entry, context)` (`session.ts:422-454`).
    pub async fn append_to_branch(
        &self,
        name: &str,
        entry: AppendEntry,
        context: Context,
    ) -> anyhow::Result<String> {
        append_to_branch(&self.core, name, entry, context).await
    }

    /// Upstream `getOrCreateBranchObject(name)` (`session.ts:456-463`).
    fn branch_object(&self, name: &str) -> Arc<dyn BranchTrait> {
        Arc::new(StorageBackedBranch {
            name: name.to_string(),
            core: Arc::clone(&self.core),
        })
    }
}

/// Upstream `StorageBackedBranch` (`session.ts:183-221`).
pub struct StorageBackedBranch {
    name: String,
    core: Arc<SessionCore>,
}

impl BranchReader for StorageBackedBranch {
    fn find_entries<'a>(
        &'a self,
        query: Option<&BranchScan>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Entry>>> {
        branch_find_entries(&self.core, &self.name, query.cloned(), context)
    }
}

/// The shared branch-ancestry scan (`session.ts:196-201`): resolve the start
/// (explicit or the branch tip), default the order to newest-first, and scan
/// storage.
fn branch_find_entries<'a>(
    core: &'a SessionCore,
    name: &'a str,
    query: Option<BranchScan>,
    context: Context,
) -> BoxFuture<'a, anyhow::Result<Vec<Entry>>> {
    Box::pin(async move {
        let query = query.unwrap_or_default();
        let start = match &query.start {
            Some(start) => start.clone(),
            None => match get_branch_tip(core, name, context.clone()).await? {
                Some(start) => start,
                None => return Ok(Vec::new()),
            },
        };
        let mut scan = StorageBranchScan::from_branch_scan(&query, start);
        scan.order = Some(query.order.unwrap_or(BranchScanOrder::NewestFirst));
        core.storage.scan_branch(&scan, context).await
    })
}

impl BranchTrait for StorageBackedBranch {
    fn name(&self) -> &str {
        &self.name
    }

    fn get_tip_id<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<Option<String>>> {
        Box::pin(async move { get_branch_tip(&self.core, &self.name, context).await })
    }

    fn find_entry<'a>(
        &'a self,
        query: Option<&BranchScan>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<Entry>>> {
        let mut limited = query.cloned().unwrap_or_default();
        limited.limit = Some(limited.limit.map_or(1, |limit| limit.min(1)));
        Box::pin(async move {
            let entries =
                branch_find_entries(&self.core, &self.name, Some(limited), context).await?;
            Ok(entries.into_iter().next())
        })
    }

    fn append_message<'a>(
        &'a self,
        message: AgentMessage,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<String>> {
        let core = Arc::clone(&self.core);
        let name = self.name.clone();
        Box::pin(async move {
            append_to_branch(
                &core,
                &name,
                AppendEntry::Message {
                    message: Box::new(message),
                },
                context,
            )
            .await
        })
    }

    fn append_custom_entry<'a>(
        &'a self,
        custom_type: String,
        data: Option<serde_json::Value>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<String>> {
        let core = Arc::clone(&self.core);
        let name = self.name.clone();
        Box::pin(async move {
            append_to_branch(
                &core,
                &name,
                AppendEntry::Custom { custom_type, data },
                context,
            )
            .await
        })
    }
}

/// Upstream `StorageBackedSessionMutation` (`session.ts:95-181`).
pub struct StorageBackedSessionMutation {
    storage: Arc<dyn Storage>,
    /// The held mutation-line guard, released by `end`.
    line_guard: Mutex<Option<tokio::sync::OwnedMutexGuard<LineState>>>,
    active: AtomicBool,
    commit_taken: AtomicBool,
    /// The settled first commit attempt, if any.
    commit_result: tokio::sync::Mutex<Option<anyhow::Result<CommitResult>>>,
}

impl StorageBackedSessionMutation {
    fn assert_active(&self) -> anyhow::Result<()> {
        if !self.active.load(Ordering::SeqCst) {
            anyhow::bail!("SessionMutator cannot be used outside its mutation callback");
        }
        Ok(())
    }

    fn reject_pending(writes: &[Write]) -> anyhow::Result<()> {
        for write in writes {
            if let Write::Entry {
                entry: NewEntry::Message { message, .. },
            } = write
            {
                assert_not_pending_assistant(message)?;
            }
        }
        Ok(())
    }
}

impl SessionMutationReader for StorageBackedSessionMutation {
    fn get_entries<'a>(
        &'a self,
        ids: &[String],
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<HashMap<String, Entry>>> {
        let ids = ids.to_vec();
        Box::pin(async move {
            self.assert_active()?;
            self.storage.get_entries(&ids, context).await
        })
    }

    fn get_stats<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<SessionStats>> {
        Box::pin(async move {
            self.assert_active()?;
            self.storage.get_stats(context).await
        })
    }

    fn get_value<'a>(
        &'a self,
        address: &ValueAddress,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<super::values::StoredValue>>> {
        let address = address.clone();
        Box::pin(async move {
            self.assert_active()?;
            self.storage.get_value(&address, context).await
        })
    }

    fn scan_values<'a>(
        &'a self,
        prefix: &ValueAddress,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<super::values::StoredValue>>> {
        let prefix = prefix.clone();
        Box::pin(async move {
            self.assert_active()?;
            self.storage.scan_values(&prefix, context).await
        })
    }

    fn read_list<'a>(
        &'a self,
        address: &ValueAddress,
        options: Option<&super::values::ListReadOptions>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<super::values::ListElement>>> {
        let address = address.clone();
        let options = options.copied();
        Box::pin(async move {
            self.assert_active()?;
            self.storage
                .read_list(&address, options.as_ref(), context)
                .await
        })
    }

    fn scan_branch<'a>(
        &'a self,
        query: &StorageBranchScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Entry>>> {
        let query = query.clone();
        Box::pin(async move {
            self.assert_active()?;
            self.storage.scan_branch(&query, context).await
        })
    }
}

impl SessionMutator for StorageBackedSessionMutation {
    fn commit<'a>(
        &'a self,
        writes: Vec<Write>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<CommitResult>> {
        Box::pin(async move {
            self.assert_active()?;
            if self.commit_taken.swap(true, Ordering::SeqCst) {
                anyhow::bail!("SessionMutator commit already attempted");
            }
            let result = match Self::reject_pending(&writes) {
                Ok(()) => self.storage.commit(writes, context).await,
                Err(error) => Err(error),
            };
            *self.commit_result.lock().await = Some(match &result {
                Ok(committed) => Ok(committed.clone()),
                Err(error) => Err(anyhow::anyhow!("{error:#}")),
            });
            result
        })
    }
}

impl SessionMutation for StorageBackedSessionMutation {
    fn end<'a>(&'a self, _context: Context) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            // Wait for the (already settled) commit attempt, invalidate the
            // capability, and release the line by dropping the guard.
            let _settled = self.commit_result.lock().await.take();
            self.active.store(false, Ordering::SeqCst);
            if let Some(guard) = self
                .line_guard
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            {
                drop(guard);
            }
            Ok(())
        })
    }
}

/// The `SessionReader` projection (Task 5).
impl SessionReader for StorageBackedSession {
    fn get_entry<'a>(
        &'a self,
        id: &str,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<Entry>>> {
        let id = id.to_string();
        Box::pin(async move {
            let mut entries =
                SessionTrait::get_entries(self, std::slice::from_ref(&id), context).await?;
            Ok(entries.remove(&id))
        })
    }
}

impl SessionTrait for StorageBackedSession {
    fn metadata(&self) -> &SessionMetadata {
        &self.metadata
    }

    fn id_generator(&self) -> Arc<dyn IdGenerator> {
        Arc::clone(&self.core.id_generator)
    }

    fn get_entries<'a>(
        &'a self,
        ids: &[String],
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<HashMap<String, Entry>>> {
        let ids = ids.to_vec();
        Box::pin(async move {
            self.assert_open()?;
            self.core.storage.get_entries(&ids, context).await
        })
    }

    fn get_stats<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<SessionStats>> {
        Box::pin(async move {
            self.assert_open()?;
            self.core.storage.get_stats(context).await
        })
    }

    fn get_value<'a>(
        &'a self,
        address: &ValueAddress,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<super::values::StoredValue>>> {
        let address = address.clone();
        Box::pin(async move {
            self.assert_open()?;
            self.core.storage.get_value(&address, context).await
        })
    }

    fn scan_values<'a>(
        &'a self,
        prefix: &ValueAddress,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<super::values::StoredValue>>> {
        let prefix = prefix.clone();
        Box::pin(async move {
            self.assert_open()?;
            self.core.storage.scan_values(&prefix, context).await
        })
    }

    fn read_list<'a>(
        &'a self,
        address: &ValueAddress,
        options: Option<&super::values::ListReadOptions>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<super::values::ListElement>>> {
        let address = address.clone();
        let options = options.copied();
        Box::pin(async move {
            self.assert_open()?;
            self.core
                .storage
                .read_list(&address, options.as_ref(), context)
                .await
        })
    }

    fn scan_branch<'a>(
        &'a self,
        query: &StorageBranchScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Entry>>> {
        let query = query.clone();
        Box::pin(async move {
            self.assert_open()?;
            self.core.storage.scan_branch(&query, context).await
        })
    }

    fn get_name<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<Option<String>>> {
        Box::pin(async move {
            Ok(SessionTrait::get_value(self, &session_name(), context)
                .await?
                .and_then(|stored| stored.value.as_str().map(str::to_string)))
        })
    }

    fn get_label<'a>(
        &'a self,
        target_id: &str,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<String>>> {
        let address = entry_label(target_id);
        Box::pin(async move {
            Ok(SessionTrait::get_value(self, &address, context)
                .await?
                .and_then(|stored| stored.value.as_str().map(str::to_string)))
        })
    }

    fn find_entries<'a>(
        &'a self,
        query: Option<&EntryQuery>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Entry>>> {
        let query = query.cloned();
        Box::pin(async move {
            self.assert_open()?;
            let query = query.unwrap_or_default();
            let order = query.order.unwrap_or(AscDescOrder::Desc);
            if let Some(cursor) = &query.cursor {
                if order == AscDescOrder::Asc && cursor.seq == MAX_SAFE_INTEGER {
                    return Ok(Vec::new());
                }
                if order == AscDescOrder::Desc && cursor.seq <= 1 {
                    return Ok(Vec::new());
                }
            }
            let mut scan = EntryScan {
                scan_type: query.scan_type,
                custom_type: query.custom_type.clone(),
                order: Some(order),
                limit: query.limit,
                from_seq: None,
                to_seq: None,
            };
            if let Some(cursor) = query.cursor {
                if order == AscDescOrder::Asc {
                    scan.from_seq = Some(cursor.seq + 1);
                } else {
                    scan.to_seq = Some(cursor.seq - 1);
                }
            }
            self.core.storage.scan_entries(&scan, context).await
        })
    }

    fn find_entry<'a>(
        &'a self,
        query: Option<&EntryQuery>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<Entry>>> {
        let mut limited = query.cloned().unwrap_or_default();
        limited.limit = Some(limited.limit.map_or(1, |limit| limit.min(1)));
        Box::pin(async move {
            Ok(SessionTrait::find_entries(self, Some(&limited), context)
                .await?
                .into_iter()
                .next())
        })
    }

    fn branch<'a>(
        &'a self,
        name: &str,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<Arc<dyn BranchTrait>>>> {
        let name = name.to_string();
        Box::pin(async move {
            Self::assert_valid_branch_name(&name)?;
            let stored = SessionTrait::get_value(self, &branch_tip(&name), context).await?;
            if stored.is_none() {
                return Ok(None);
            }
            Ok(Some(self.branch_object(&name)))
        })
    }

    fn create_branch<'a>(
        &'a self,
        name: &str,
        at: Option<String>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Arc<dyn BranchTrait>>> {
        let name = name.to_string();
        let name_for_object = name.clone();
        Box::pin(async move {
            self.assert_open()?;
            Self::assert_valid_branch_name(&name)?;
            let core = Arc::clone(&self.core);
            run_mutation(
                &core,
                |mutator, context| {
                    let name = name.clone();
                    let at = at.clone();
                    Box::pin(async move {
                        let exists = mutator
                            .get_value(&branch_tip(&name), context.clone())
                            .await?;
                        if exists.is_some() {
                            return Err(anyhow::Error::new(SessionBranchExistsError(name.clone())));
                        }
                        if let Some(at) = &at {
                            let entries = mutator
                                .get_entries(std::slice::from_ref(at), context.clone())
                                .await?;
                            if !entries.contains_key(at) {
                                return Err(anyhow::Error::new(SessionUnknownTargetError(
                                    at.clone(),
                                )));
                            }
                        }
                        mutator
                            .commit(
                                vec![set_value_write(
                                    &branch_tip(&name),
                                    at.map(serde_json::Value::String)
                                        .unwrap_or(serde_json::Value::Null),
                                )],
                                context.clone(),
                            )
                            .await?;
                        Ok(())
                    })
                },
                context,
            )
            .await?;
            Ok(self.branch_object(&name_for_object))
        })
    }

    fn begin_mutation<'a>(
        &'a self,
        _context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Box<dyn SessionMutation>>> {
        let core = Arc::clone(&self.core);
        Box::pin(async move { begin_session_mutation(&core).await })
    }

    fn set_value<'a>(
        &'a self,
        address: ValueAddress,
        next: serde_json::Value,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        let core = Arc::clone(&self.core);
        Box::pin(async move {
            run_mutation(
                &core,
                |mutator, context| {
                    Box::pin(async move {
                        mutator
                            .commit(vec![set_value_write(&address, next)], context.clone())
                            .await?;
                        Ok(())
                    })
                },
                context,
            )
            .await
        })
    }

    fn delete_value<'a>(
        &'a self,
        address: ValueAddress,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        let core = Arc::clone(&self.core);
        Box::pin(async move {
            run_mutation(
                &core,
                |mutator, context| {
                    Box::pin(async move {
                        mutator
                            .commit(vec![delete_value_write(&address)], context.clone())
                            .await?;
                        Ok(())
                    })
                },
                context,
            )
            .await
        })
    }

    fn append_list<'a>(
        &'a self,
        address: ValueAddress,
        element: serde_json::Value,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        let core = Arc::clone(&self.core);
        Box::pin(async move {
            run_mutation(
                &core,
                |mutator, context| {
                    Box::pin(async move {
                        mutator
                            .commit(vec![append_list_write(&address, element)], context.clone())
                            .await?;
                        Ok(())
                    })
                },
                context,
            )
            .await
        })
    }

    fn delete_list<'a>(
        &'a self,
        address: ValueAddress,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        let core = Arc::clone(&self.core);
        Box::pin(async move {
            run_mutation(
                &core,
                |mutator, context| {
                    Box::pin(async move {
                        mutator
                            .commit(vec![delete_list_write(&address)], context.clone())
                            .await?;
                        Ok(())
                    })
                },
                context,
            )
            .await
        })
    }

    fn set_name<'a>(
        &'a self,
        name: Option<String>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            match name {
                None => SessionTrait::delete_value(self, session_name(), context).await,
                Some(name) => {
                    SessionTrait::set_value(
                        self,
                        session_name(),
                        serde_json::Value::String(name),
                        context,
                    )
                    .await
                }
            }
        })
    }

    fn set_label<'a>(
        &'a self,
        target_id: &str,
        label: Option<String>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        let target_id = target_id.to_string();
        Box::pin(async move {
            let address = entry_label(&target_id);
            match label {
                None => SessionTrait::delete_value(self, address, context).await,
                Some(label) => {
                    SessionTrait::set_value(
                        self,
                        address,
                        serde_json::Value::String(label),
                        context,
                    )
                    .await
                }
            }
        })
    }

    fn close<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let core = Arc::clone(&self.core);
            let core_for_init = Arc::clone(&core);
            core.close_gate
                .get_or_init(|| async move {
                    let core = core_for_init;
                    core.state.store(CLOSING, Ordering::SeqCst);
                    core.mutation_line.seal(CLOSED_ERROR).await;
                    let _ = core.storage.close(context).await;
                    core.state.store(CLOSED, Ordering::SeqCst);
                    if let Some(on_close) = &core.on_close {
                        on_close();
                    }
                })
                .await;
            Ok(())
        })
    }
}

/// Compile-time check that the concrete branch satisfies the Task 5
/// projection (`Pick<Branch, "findEntries">`).
const _: fn(&StorageBackedBranch) -> &dyn BranchReader = |branch| branch;
/// Compile-time check that the concrete session satisfies the Task 5
/// projection (`Pick<Session, "getEntry">`).
const _: fn(&StorageBackedSession) -> &dyn SessionReader = |session| session;

#[cfg(test)]
mod tests;
