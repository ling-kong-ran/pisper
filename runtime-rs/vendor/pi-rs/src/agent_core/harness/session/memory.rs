//! Port of `packages/agent/src/harness/session/memory.ts` (453 lines): the
//! in-memory [`Storage`] backend over [`InMemoryStorageState`], with the same
//! admission lifecycle (commits admitted before `close` drain; every later
//! operation rejects), the [`MemoryStorage::fork`] state fork, and the
//! [`MemorySessionRepo`] / [`MemorySessionFacade`] repo layer over
//! `StorageBackedSession` (`memory.ts:136-453`).
//!
//! Substitutions: upstream promise admission bookkeeping becomes an in-flight
//! counter drained by `close` (an un-ended abandoned mutation no longer
//! blocks close once its handle drops — disclosed divergence from
//! `allSettled(admitted)`); `list` is a sync inherent method (no `await`,
//! unlike the upstream async signature). Oracle: `memory.ts` read in full;
//! `memory-session-repo.test.ts` / `memory-conformance.test.ts` referenced —
//! byte-level replay of those files is still outstanding (M3b Task 11).

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;

use crate::agent_core::chord_support::Context;

use super::session::StorageBackedSession;
use super::storage_state::InMemoryStorageState;
use super::types::{
    Branch, BranchReader, CommitResult, Entry, EntryQuery, EntryScan, EntryStructure, ForkOptions,
    Session, SessionCreateOptions, SessionMetadata, SessionMutation, SessionMutationReader,
    SessionMutator, SessionStats, Storage, StorageBranchScan, UsageRow, UsageScan, Write,
};
use super::values::{ListElement, ListReadOptions, StoredValue, ValueAddress};

/// Upstream lifecycle literals (`memory.ts:41`).
const OPEN: u8 = 0;
const CLOSING: u8 = 1;
const CLOSED: u8 = 2;

/// Upstream `MemoryStorage` (`memory.ts:37-134`).
pub struct MemoryStorage {
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
    /// The serialized commit/apply region (upstream `commitQueue`).
    inner: tokio::sync::Mutex<InMemoryStorageState>,
    /// Upstream `state: "open" | "closing" | "closed"`.
    lifecycle: AtomicU8,
    /// Upstream `closePromise` — one shared close completion.
    close_gate: tokio::sync::OnceCell<()>,
}

impl std::fmt::Debug for MemoryStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryStorage").finish_non_exhaustive()
    }
}

/// Upstream `MemoryStorageOptions` (`memory.ts:29-31`).
#[derive(Default)]
pub struct MemoryStorageOptions {
    /// Upstream `now?: () => number`. Defaults to the wall clock.
    pub now: Option<Arc<dyn Fn() -> i64 + Send + Sync>>,
}

impl MemoryStorage {
    /// Upstream `new MemoryStorage(options)` (`memory.ts:44-46`).
    pub fn new(options: MemoryStorageOptions) -> Self {
        MemoryStorage {
            now: options.now.unwrap_or_else(|| Arc::new(crate::ai::now_ms)),
            inner: tokio::sync::Mutex::new(InMemoryStorageState::new()),
            lifecycle: AtomicU8::new(OPEN),
            close_gate: tokio::sync::OnceCell::new(),
        }
    }

    /// Upstream `fork(options)` (`memory.ts:111-124`): construct a
    /// destination storage at one serialized boundary between source commits.
    pub async fn fork(&self, options: &ForkOptions) -> anyhow::Result<MemoryStorage> {
        if self.lifecycle.load(Ordering::SeqCst) != OPEN {
            anyhow::bail!("MemoryStorage is closed");
        }
        // Hold the source lock across the fork, matching the upstream
        // commit-queue boundary ("at one serialized boundary between source
        // commits").
        let source = self.inner.lock().await;
        let forked = source.create_fork(options)?;
        drop(source);
        Ok(MemoryStorage {
            now: Arc::clone(&self.now),
            inner: tokio::sync::Mutex::new(forked),
            lifecycle: AtomicU8::new(OPEN),
            close_gate: tokio::sync::OnceCell::new(),
        })
    }

    fn assert_open(&self) -> anyhow::Result<()> {
        if self.lifecycle.load(Ordering::SeqCst) != OPEN {
            anyhow::bail!("MemoryStorage is closed");
        }
        Ok(())
    }
}

impl Storage for MemoryStorage {
    fn commit<'a>(
        &'a self,
        writes: Vec<Write>,
        _context: Context,
    ) -> BoxFuture<'a, anyhow::Result<CommitResult>> {
        Box::pin(async move {
            self.assert_open()?;
            let timestamp = (self.now)();
            let mut state = self.inner.lock().await;
            let prepared = state.prepare_commit(writes, timestamp)?;
            let stats = state.apply_validated(prepared.writes);
            Ok(prepared.result.with_stats(stats))
        })
    }

    fn get_entries<'a>(
        &'a self,
        ids: &[String],
        _context: Context,
    ) -> BoxFuture<'a, anyhow::Result<std::collections::HashMap<String, Entry>>> {
        let ids = ids.to_vec();
        Box::pin(async move {
            self.assert_open()?;
            let state = self.inner.lock().await;
            Ok(state.get_entries(&ids))
        })
    }

    fn get_value<'a>(
        &'a self,
        address: &ValueAddress,
        _context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<StoredValue>>> {
        let address = address.clone();
        Box::pin(async move {
            self.assert_open()?;
            let state = self.inner.lock().await;
            Ok(state.get_value(&address))
        })
    }

    fn scan_values<'a>(
        &'a self,
        prefix: &ValueAddress,
        _context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<StoredValue>>> {
        let prefix = prefix.clone();
        Box::pin(async move {
            self.assert_open()?;
            let state = self.inner.lock().await;
            Ok(state.scan_values(&prefix))
        })
    }

    fn read_list<'a>(
        &'a self,
        address: &ValueAddress,
        options: Option<&ListReadOptions>,
        _context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<ListElement>>> {
        let address = address.clone();
        let options = options.copied();
        Box::pin(async move {
            self.assert_open()?;
            let state = self.inner.lock().await;
            state.read_list(&address, options.as_ref())
        })
    }

    fn scan_branch<'a>(
        &'a self,
        query: &StorageBranchScan,
        _context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Entry>>> {
        let query = query.clone();
        Box::pin(async move {
            self.assert_open()?;
            let state = self.inner.lock().await;
            state.scan_branch(&query)
        })
    }

    fn scan_branch_structure<'a>(
        &'a self,
        query: &StorageBranchScan,
        _context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<EntryStructure>>> {
        let query = query.clone();
        Box::pin(async move {
            self.assert_open()?;
            let state = self.inner.lock().await;
            state.scan_branch_structure(&query)
        })
    }

    fn scan_entries<'a>(
        &'a self,
        query: &EntryScan,
        _context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Entry>>> {
        let query = query.clone();
        Box::pin(async move {
            self.assert_open()?;
            let state = self.inner.lock().await;
            Ok(state.scan_entries(&query))
        })
    }

    fn scan_usage<'a>(
        &'a self,
        query: &UsageScan,
        _context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<UsageRow>>> {
        let query = query.clone();
        Box::pin(async move {
            self.assert_open()?;
            let state = self.inner.lock().await;
            Ok(state.scan_usage(&query))
        })
    }

    fn get_stats<'a>(&'a self, _context: Context) -> BoxFuture<'a, anyhow::Result<SessionStats>> {
        Box::pin(async move {
            self.assert_open()?;
            let state = self.inner.lock().await;
            Ok(state.get_stats())
        })
    }

    fn close<'a>(&'a self, _context: Context) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            // Upstream: the first close() sets "closing", chains behind the
            // commit queue, then sets "closed"; every caller shares one
            // completion. Committed-but-undrained commits are impossible
            // here because admission is checked before the queue.
            self.close_gate
                .get_or_init(|| async move {
                    self.lifecycle.store(CLOSING, Ordering::SeqCst);
                    let _drain = self.inner.lock().await;
                    self.lifecycle.store(CLOSED, Ordering::SeqCst);
                })
                .await;
            Ok(())
        })
    }
}

/// Upstream `MEMORY_STORAGE_VERSION` (`memory.ts:136`).
const MEMORY_STORAGE_VERSION: i64 = 1;

/// Shared admission state for one open session facade: the upstream
/// `admitted` handle set plus the `"open" | "closing" | "closed"` lifecycle
/// (`memory.ts:150-153`).
struct Admission {
    lifecycle: AtomicU8,
    /// Admitted operations that have not settled yet.
    inflight: AtomicUsize,
    /// Signalled every time `inflight` decreases (close drains on this).
    drained: tokio::sync::Notify,
}

impl Admission {
    fn new() -> Arc<Self> {
        Arc::new(Admission {
            lifecycle: AtomicU8::new(OPEN),
            inflight: AtomicUsize::new(0),
            drained: tokio::sync::Notify::new(),
        })
    }

    /// Upstream `admit` entry (`memory.ts:317-331`): register the operation
    /// handle, then reject if closed. Enter-then-check keeps the concurrent
    /// close race honest — an operation admitted before `closing` runs and
    /// drains; one admitted after unwinds with the upstream closed error.
    fn enter(self: &Arc<Self>) -> anyhow::Result<AdmissionGuard> {
        self.inflight.fetch_add(1, Ordering::SeqCst);
        if self.lifecycle.load(Ordering::SeqCst) != OPEN {
            self.leave();
            anyhow::bail!("Session is closed");
        }
        Ok(AdmissionGuard {
            admission: Arc::clone(self),
        })
    }

    fn leave(&self) {
        if self.inflight.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.drained.notify_waiters();
        }
    }

    /// Upstream `close` waits `allSettled(admitted)`: no new admissions once
    /// `closing`, and the drain finishes when the last in-flight handle drops.
    async fn drain(&self) {
        loop {
            if self.inflight.load(Ordering::SeqCst) == 0 {
                return;
            }
            let notified = self.drained.notified();
            if self.inflight.load(Ordering::SeqCst) == 0 {
                return;
            }
            notified.await;
        }
    }
}

/// One admitted operation; dropping it settles the handle.
struct AdmissionGuard {
    admission: Arc<Admission>,
}

impl std::fmt::Debug for MemorySessionFacade {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemorySessionFacade")
            .field("id", &self.session.metadata().id)
            .finish_non_exhaustive()
    }
}

impl Drop for AdmissionGuard {
    fn drop(&mut self) {
        self.admission.leave();
    }
}

fn is_open(admission: &Admission) -> bool {
    admission.lifecycle.load(Ordering::SeqCst) == OPEN
}

/// Upstream `MemorySessionFacade` (`memory.ts:145-332`): one open [`Session`]
/// handle over a [`StorageBackedSession`], gating every operation through the
/// shared [`Admission`] and flipping the repo record's `open` flag on close.
pub struct MemorySessionFacade {
    session: Arc<StorageBackedSession>,
    /// Upstream `onClose` flips `record.open` (`memory.ts:440-442`).
    open_flag: Arc<AtomicBool>,
    admission: Arc<Admission>,
    /// Upstream `closePromise` — one shared close completion.
    close_gate: tokio::sync::OnceCell<()>,
}

impl Session for MemorySessionFacade {
    fn metadata(&self) -> &SessionMetadata {
        self.session.metadata()
    }

    fn id_generator(&self) -> Arc<dyn super::types::IdGenerator> {
        self.session.id_generator()
    }

    fn get_entries<'a>(
        &'a self,
        ids: &[String],
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<std::collections::HashMap<String, Entry>>> {
        let ids = ids.to_vec();
        Box::pin(async move {
            let _admitted = self.admission.enter()?;
            self.session.get_entries(&ids, context).await
        })
    }

    fn get_stats<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<SessionStats>> {
        Box::pin(async move {
            let _admitted = self.admission.enter()?;
            self.session.get_stats(context).await
        })
    }

    fn get_value<'a>(
        &'a self,
        address: &ValueAddress,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<StoredValue>>> {
        let address = address.clone();
        Box::pin(async move {
            let _admitted = self.admission.enter()?;
            self.session.get_value(&address, context).await
        })
    }

    fn scan_values<'a>(
        &'a self,
        prefix: &ValueAddress,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<StoredValue>>> {
        let prefix = prefix.clone();
        Box::pin(async move {
            let _admitted = self.admission.enter()?;
            self.session.scan_values(&prefix, context).await
        })
    }

    fn read_list<'a>(
        &'a self,
        address: &ValueAddress,
        options: Option<&ListReadOptions>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<ListElement>>> {
        let address = address.clone();
        let options = options.copied();
        Box::pin(async move {
            let _admitted = self.admission.enter()?;
            self.session
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
            let _admitted = self.admission.enter()?;
            self.session.scan_branch(&query, context).await
        })
    }

    fn get_name<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<Option<String>>> {
        Box::pin(async move {
            let _admitted = self.admission.enter()?;
            self.session.get_name(context).await
        })
    }

    fn get_label<'a>(
        &'a self,
        target_id: &str,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<String>>> {
        let target_id = target_id.to_string();
        Box::pin(async move {
            let _admitted = self.admission.enter()?;
            self.session.get_label(&target_id, context).await
        })
    }

    fn find_entries<'a>(
        &'a self,
        query: Option<&EntryQuery>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Entry>>> {
        let query = query.cloned();
        Box::pin(async move {
            let _admitted = self.admission.enter()?;
            self.session.find_entries(query.as_ref(), context).await
        })
    }

    fn find_entry<'a>(
        &'a self,
        query: Option<&EntryQuery>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<Entry>>> {
        let query = query.cloned();
        Box::pin(async move {
            let _admitted = self.admission.enter()?;
            self.session.find_entry(query.as_ref(), context).await
        })
    }

    fn branch<'a>(
        &'a self,
        name: &str,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<Arc<dyn Branch>>>> {
        let name = name.to_string();
        Box::pin(async move {
            let _admitted = self.admission.enter()?;
            let branch = self.session.branch(&name, context).await?;
            Ok(branch.map(|branch| self.wrap_branch(branch)))
        })
    }

    fn create_branch<'a>(
        &'a self,
        name: &str,
        at: Option<String>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Arc<dyn Branch>>> {
        let name = name.to_string();
        Box::pin(async move {
            let _admitted = self.admission.enter()?;
            let branch = self.session.create_branch(&name, at, context).await?;
            Ok(self.wrap_branch(branch))
        })
    }

    /// Upstream `beginMutation` (`memory.ts:162-203`): the mutation handle is
    /// admitted before `begin` and released at `end` (or on failure); a close
    /// that lands between `begin` and the open check ends the source
    /// capability and rejects.
    fn begin_mutation<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Box<dyn SessionMutation>>> {
        Box::pin(async move {
            let guard = self.admission.enter()?;
            let source = match self.session.begin_mutation(context.clone()).await {
                Ok(source) => source,
                Err(error) => {
                    drop(guard);
                    return Err(error);
                }
            };
            if !is_open(&self.admission) {
                let _ended = source.end(context).await;
                drop(guard);
                anyhow::bail!("Session is closed");
            }
            Ok(Box::new(MemorySessionMutation {
                source,
                guard: Mutex::new(Some(guard)),
            }) as Box<dyn SessionMutation>)
        })
    }

    /// Upstream `mutate` (`memory.ts:205-212`): admitted for the whole
    /// callback; the closed check runs when the callback starts, not only at
    /// admission.
    fn mutate<'a, T, F>(&'a self, mutation: F, context: Context) -> BoxFuture<'a, anyhow::Result<T>>
    where
        F: FnOnce(&dyn SessionMutator, Context) -> BoxFuture<'_, anyhow::Result<T>> + Send + 'a,
        T: Send + 'a,
    {
        Box::pin(async move {
            let admitted = self.admission.enter()?;
            let admission = Arc::clone(&self.admission);
            let result = self
                .session
                .mutate(
                    |mutator, context| {
                        if !is_open(&admission) {
                            return Box::pin(async { anyhow::bail!("Session is closed") })
                                as BoxFuture<'_, anyhow::Result<T>>;
                        }
                        mutation(mutator, context)
                    },
                    context,
                )
                .await;
            drop(admitted);
            result
        })
    }

    fn set_value<'a>(
        &'a self,
        address: ValueAddress,
        next: serde_json::Value,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let _admitted = self.admission.enter()?;
            self.session.set_value(address, next, context).await
        })
    }

    fn delete_value<'a>(
        &'a self,
        address: ValueAddress,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let _admitted = self.admission.enter()?;
            self.session.delete_value(address, context).await
        })
    }

    fn append_list<'a>(
        &'a self,
        address: ValueAddress,
        element: serde_json::Value,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let _admitted = self.admission.enter()?;
            self.session.append_list(address, element, context).await
        })
    }

    fn delete_list<'a>(
        &'a self,
        address: ValueAddress,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let _admitted = self.admission.enter()?;
            self.session.delete_list(address, context).await
        })
    }

    fn set_name<'a>(
        &'a self,
        name: Option<String>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let _admitted = self.admission.enter()?;
            self.session.set_name(name, context).await
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
            let _admitted = self.admission.enter()?;
            self.session.set_label(&target_id, label, context).await
        })
    }

    /// Upstream facade `close` (`memory.ts:295-303`): wait for admitted
    /// operations, then mark closed and flip the repo record's `open` flag.
    /// The inner [`StorageBackedSession`] stays open — the repo reopens it.
    fn close<'a>(&'a self, _context: Context) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let admission = Arc::clone(&self.admission);
            let open_flag = Arc::clone(&self.open_flag);
            self.close_gate
                .get_or_init(move || async move {
                    admission.lifecycle.store(CLOSING, Ordering::SeqCst);
                    admission.drain().await;
                    admission.lifecycle.store(CLOSED, Ordering::SeqCst);
                    open_flag.store(false, Ordering::SeqCst);
                })
                .await;
            Ok(())
        })
    }
}

/// Upstream `getEntry` comes from the `SessionReader` supertrait
/// (`memory.ts:218-220` — admitted like every read).
impl super::types::SessionReader for MemorySessionFacade {
    fn get_entry<'a>(
        &'a self,
        id: &str,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<Entry>>> {
        let id = id.to_string();
        Box::pin(async move {
            let _admitted = self.admission.enter()?;
            self.session.get_entry(&id, context).await
        })
    }
}

impl MemorySessionFacade {
    /// Upstream `wrapBranch` (`memory.ts:305-315`): every branch operation
    /// re-enters admission at call time; `name` is a passthrough read.
    fn wrap_branch(&self, branch: Arc<dyn Branch>) -> Arc<dyn Branch> {
        Arc::new(MemoryBranchFacade {
            branch,
            admission: Arc::clone(&self.admission),
        })
    }
}

/// Upstream `SessionMutation` projection (`memory.ts:183-202`): reads and
/// `commit` delegate straight to the source capability; `end` releases the
/// admitted handle (upstream `finally`).
struct MemorySessionMutation {
    source: Box<dyn SessionMutation>,
    guard: Mutex<Option<AdmissionGuard>>,
}

impl SessionMutationReader for MemorySessionMutation {
    fn get_entries<'a>(
        &'a self,
        ids: &[String],
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<std::collections::HashMap<String, Entry>>> {
        self.source.get_entries(ids, context)
    }

    fn get_stats<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<SessionStats>> {
        self.source.get_stats(context)
    }

    fn get_value<'a>(
        &'a self,
        address: &ValueAddress,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<StoredValue>>> {
        self.source.get_value(address, context)
    }

    fn scan_values<'a>(
        &'a self,
        prefix: &ValueAddress,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<StoredValue>>> {
        self.source.scan_values(prefix, context)
    }

    fn read_list<'a>(
        &'a self,
        address: &ValueAddress,
        options: Option<&ListReadOptions>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<ListElement>>> {
        self.source.read_list(address, options, context)
    }

    fn scan_branch<'a>(
        &'a self,
        query: &StorageBranchScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Entry>>> {
        self.source.scan_branch(query, context)
    }
}

impl SessionMutator for MemorySessionMutation {
    fn commit<'a>(
        &'a self,
        writes: Vec<Write>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<CommitResult>> {
        self.source.commit(writes, context)
    }
}

impl SessionMutation for MemorySessionMutation {
    fn end<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let result = self.source.end(context).await;
            if let Ok(mut guard) = self.guard.lock() {
                guard.take();
            }
            result
        })
    }
}

/// Upstream branch wrapper (`memory.ts:305-315`).
struct MemoryBranchFacade {
    branch: Arc<dyn Branch>,
    admission: Arc<Admission>,
}

impl BranchReader for MemoryBranchFacade {
    fn find_entries<'a>(
        &'a self,
        query: Option<&super::types::BranchScan>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Entry>>> {
        let query = query.cloned();
        Box::pin(async move {
            let _admitted = self.admission.enter()?;
            self.branch.find_entries(query.as_ref(), context).await
        })
    }
}

impl Branch for MemoryBranchFacade {
    fn name(&self) -> &str {
        self.branch.name()
    }

    fn get_tip_id<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<Option<String>>> {
        Box::pin(async move {
            let _admitted = self.admission.enter()?;
            self.branch.get_tip_id(context).await
        })
    }

    fn find_entry<'a>(
        &'a self,
        query: Option<&super::types::BranchScan>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<Entry>>> {
        let query = query.cloned();
        Box::pin(async move {
            let _admitted = self.admission.enter()?;
            self.branch.find_entry(query.as_ref(), context).await
        })
    }

    fn append_message<'a>(
        &'a self,
        message: crate::agent_core::types::AgentMessage,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<String>> {
        Box::pin(async move {
            let _admitted = self.admission.enter()?;
            self.branch.append_message(message, context).await
        })
    }

    fn append_custom_entry<'a>(
        &'a self,
        custom_type: String,
        data: Option<serde_json::Value>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<String>> {
        Box::pin(async move {
            let _admitted = self.admission.enter()?;
            self.branch
                .append_custom_entry(custom_type, data, context)
                .await
        })
    }
}

/// Upstream `MemorySessionRecord` (`memory.ts:138-143`).
struct MemorySessionRecord {
    metadata: SessionMetadata,
    storage: Arc<MemoryStorage>,
    session: Arc<StorageBackedSession>,
    /// Flipped by the facade close (`record.open`).
    open: Arc<AtomicBool>,
}

/// Upstream `MemorySessionRepoOptions` (`memory.ts:33-35`).
#[derive(Default)]
pub struct MemorySessionRepoOptions {
    /// Upstream `now?: () => number`. Defaults to the wall clock.
    pub now: Option<Arc<dyn Fn() -> i64 + Send + Sync>>,
}

/// Upstream `MemorySessionRepo` (`memory.ts:334-453`). Like the JSONL repo,
/// the generic upstream `SessionRepo` interface lands as inherent methods.
pub struct MemorySessionRepo {
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
    /// Upstream insertion-ordered `Map`; a `Vec` preserves the order.
    sessions: Mutex<Vec<MemorySessionRecord>>,
    pending_ids: Mutex<HashSet<String>>,
    closed: AtomicBool,
    /// Upstream `closePromise` — one shared close completion.
    close_gate: tokio::sync::OnceCell<()>,
}

impl std::fmt::Debug for MemorySessionRepo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemorySessionRepo").finish_non_exhaustive()
    }
}

impl MemorySessionRepo {
    /// Upstream `new MemorySessionRepo(options)` (`memory.ts:341-343`).
    pub fn new(options: MemorySessionRepoOptions) -> Self {
        MemorySessionRepo {
            now: options.now.unwrap_or_else(|| Arc::new(crate::ai::now_ms)),
            sessions: Mutex::new(Vec::new()),
            pending_ids: Mutex::new(HashSet::new()),
            closed: AtomicBool::new(false),
            close_gate: tokio::sync::OnceCell::new(),
        }
    }

    fn assert_open(&self) -> anyhow::Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            anyhow::bail!("MemorySessionRepo is closed");
        }
        Ok(())
    }

    /// Upstream `reserveId` (`memory.ts:445-448`).
    fn reserve_id(&self, id: &str) -> anyhow::Result<()> {
        let sessions = self.lock_sessions();
        let mut pending = self.lock_pending();
        if sessions.iter().any(|record| record.metadata.id == id) || pending.contains(id) {
            anyhow::bail!("Session already exists: {id}");
        }
        pending.insert(id.to_string());
        Ok(())
    }

    fn unreserve_id(&self, id: &str) {
        self.lock_pending().remove(id);
    }

    fn lock_sessions(&self) -> std::sync::MutexGuard<'_, Vec<MemorySessionRecord>> {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn lock_pending(&self) -> std::sync::MutexGuard<'_, HashSet<String>> {
        self.pending_ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn new_session_id(&self, created_at: i64, requested: Option<&String>) -> String {
        match requested {
            Some(id) => id.clone(),
            None => crate::ai::uuid::uuid_v7_at(created_at).expect("uuidv7 timestamp in range"),
        }
    }

    /// Upstream `openRecord` (`memory.ts:439-443`).
    fn open_record(
        &self,
        session: Arc<StorageBackedSession>,
        open: Arc<AtomicBool>,
    ) -> Arc<MemorySessionFacade> {
        Arc::new(MemorySessionFacade {
            session,
            open_flag: open,
            admission: Admission::new(),
            close_gate: tokio::sync::OnceCell::new(),
        })
    }

    /// Upstream `create` (`memory.ts:345-373`). `context` mirrors the
    /// upstream signature — it reaches the error-path `session.close`, which
    /// this infallible construction sequence cannot enter.
    pub async fn create(
        &self,
        options: SessionCreateOptions,
        _context: Context,
    ) -> anyhow::Result<Arc<MemorySessionFacade>> {
        self.assert_open()?;
        let created_at = (self.now)();
        let id = self.new_session_id(created_at, options.id.as_ref());
        self.reserve_id(&id)?;
        let result = async {
            let metadata = SessionMetadata {
                id: id.clone(),
                created_at,
                storage_version: MEMORY_STORAGE_VERSION,
                parent_session_id: options.parent_session_id.clone(),
                ..SessionMetadata::default()
            };
            let storage = Arc::new(MemoryStorage::new(MemoryStorageOptions {
                now: Some(Arc::clone(&self.now)),
            }));
            let session = Arc::new(StorageBackedSession::new(
                metadata.clone(),
                Arc::clone(&storage) as Arc<dyn Storage>,
            ));
            let open = Arc::new(AtomicBool::new(true));
            self.lock_sessions().push(MemorySessionRecord {
                metadata,
                storage,
                session: Arc::clone(&session),
                open: Arc::clone(&open),
            });
            Ok(self.open_record(session, open))
        }
        .await;
        self.unreserve_id(&id);
        result
    }

    /// Upstream `open` (`memory.ts:375-384`): memory sessions are always at
    /// the current storage version, so no persistent-backend gating applies.
    pub async fn open(
        &self,
        metadata: &SessionMetadata,
        _context: Context,
    ) -> anyhow::Result<Arc<MemorySessionFacade>> {
        self.assert_open()?;
        let mut sessions = self.lock_sessions();
        let record = sessions
            .iter_mut()
            .find(|record| record.metadata.id == metadata.id)
            .ok_or_else(|| anyhow::anyhow!("Unknown session: {}", metadata.id))?;
        if record.open.load(Ordering::SeqCst) {
            anyhow::bail!("Session is already open: {}", metadata.id);
        }
        record.open.store(true, Ordering::SeqCst);
        Ok(self.open_record(Arc::clone(&record.session), Arc::clone(&record.open)))
    }

    /// Upstream `list` (`memory.ts:386-389`) in insertion order. Sync here —
    /// there is no await (disclosed signature substitution).
    pub fn list(&self) -> anyhow::Result<Vec<SessionMetadata>> {
        self.assert_open()?;
        Ok(self
            .lock_sessions()
            .iter()
            .map(|record| record.metadata.clone())
            .collect())
    }

    /// Upstream `delete` (`memory.ts:391-398`).
    pub async fn delete(&self, metadata: &SessionMetadata, context: Context) -> anyhow::Result<()> {
        self.assert_open()?;
        let session = {
            let sessions = self.lock_sessions();
            let record = sessions
                .iter()
                .find(|record| record.metadata.id == metadata.id)
                .ok_or_else(|| anyhow::anyhow!("Unknown session: {}", metadata.id))?;
            if record.open.load(Ordering::SeqCst) {
                anyhow::bail!("Session is open: {}", metadata.id);
            }
            Arc::clone(&record.session)
        };
        session.close(context).await?;
        self.lock_sessions()
            .retain(|record| record.metadata.id != metadata.id);
        Ok(())
    }

    /// Upstream `fork` (`memory.ts:400-428`): the destination storage is
    /// constructed at one serialized boundary of the source storage.
    pub async fn fork(
        &self,
        source: &SessionMetadata,
        options: &ForkOptions,
        _context: Context,
    ) -> anyhow::Result<Arc<MemorySessionFacade>> {
        self.assert_open()?;
        let (source_storage, source_id) = {
            let sessions = self.lock_sessions();
            let record = sessions
                .iter()
                .find(|record| record.metadata.id == source.id)
                .ok_or_else(|| anyhow::anyhow!("Unknown session: {}", source.id))?;
            (Arc::clone(&record.storage), record.metadata.id.clone())
        };
        let created_at = (self.now)();
        let requested = match options {
            ForkOptions::Branch { id, .. } | ForkOptions::Tree { id } => id.clone(),
        };
        let id = self.new_session_id(created_at, requested.as_ref());
        self.reserve_id(&id)?;
        let result = async {
            let storage = Arc::new(source_storage.fork(options).await?);
            let metadata = SessionMetadata {
                id: id.clone(),
                created_at,
                storage_version: MEMORY_STORAGE_VERSION,
                parent_session_id: Some(source_id),
                ..SessionMetadata::default()
            };
            let session = Arc::new(StorageBackedSession::new(
                metadata.clone(),
                Arc::clone(&storage) as Arc<dyn Storage>,
            ));
            let open = Arc::new(AtomicBool::new(true));
            self.lock_sessions().push(MemorySessionRecord {
                metadata,
                storage,
                session: Arc::clone(&session),
                open: Arc::clone(&open),
            });
            Ok(self.open_record(session, open))
        }
        .await;
        self.unreserve_id(&id);
        result
    }

    /// Upstream `close` (`memory.ts:430-437`): closed immediately, then every
    /// backing session closes; errors are swallowed like the upstream
    /// `Promise.all(...).then(() => undefined)`.
    pub async fn close(&self, context: Context) -> anyhow::Result<()> {
        let sessions = {
            let guard = self.lock_sessions();
            guard
                .iter()
                .map(|record| Arc::clone(&record.session))
                .collect::<Vec<_>>()
        };
        self.close_gate
            .get_or_init(|| async move {
                self.closed.store(true, Ordering::SeqCst);
                for session in &sessions {
                    let _closed = session.close(context.clone()).await;
                }
            })
            .await;
        Ok(())
    }
}

#[cfg(test)]
mod repo_tests;

#[cfg(test)]
mod tests;
