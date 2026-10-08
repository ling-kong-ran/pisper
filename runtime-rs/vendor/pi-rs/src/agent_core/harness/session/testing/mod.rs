//! Port of `packages/agent/src/harness/session/testing/` (the test-support
//! subset the M3b Task 7 oracles use): [`GatingStorage`] and
//! [`InstrumentedStorage`] over the decorator base.
//!
//! Disclosed substitutions:
//! - Upstream `StorageDecorator` (an inheritance base) becomes a private
//!   forwarding struct; the two concrete decorators wrap
//!   `Arc<dyn Storage>` directly.
//! - The upstream `conformance/` suites (storage.ts 920, session-repo.ts
//!   1185) exist to parametrize identical assertions across Memory/JSONL/
//!   SQLite backends and a runner-independent case list. This phase only
//!   instantiates them for the JSONL backend, so the case bodies are ported
//!   as flat `#[tokio::test]` functions under the oracle test modules
//!   (`jsonl::tests`, `memory::tests`, `repo tests`) with the same groups,
//!   names, and assertions — the runner machinery itself is not ported.

use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::session::types::{
    CommitResult, Entry, EntryScan, EntryStructure, SessionStats, Storage, StorageBranchScan,
    UsageRow, UsageScan, Write,
};
use crate::agent_core::harness::session::values::{
    ListElement, ListReadOptions, StoredValue, ValueAddress,
};

/// Upstream `StorageDecorator` (`testing/storage-decorator.ts`): the shared
/// forwarding base of the two test decorators.
#[derive(Clone)]
pub(crate) struct DecoratorStorage {
    pub delegate: Arc<dyn Storage>,
}

impl Storage for DecoratorStorage {
    fn commit<'a>(
        &'a self,
        writes: Vec<Write>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<CommitResult>> {
        self.delegate.commit(writes, context)
    }

    fn get_entries<'a>(
        &'a self,
        ids: &[String],
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<std::collections::HashMap<String, Entry>>> {
        self.delegate.get_entries(ids, context)
    }

    fn get_value<'a>(
        &'a self,
        address: &ValueAddress,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<StoredValue>>> {
        self.delegate.get_value(address, context)
    }

    fn scan_values<'a>(
        &'a self,
        prefix: &ValueAddress,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<StoredValue>>> {
        self.delegate.scan_values(prefix, context)
    }

    fn read_list<'a>(
        &'a self,
        address: &ValueAddress,
        options: Option<&ListReadOptions>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<ListElement>>> {
        self.delegate.read_list(address, options, context)
    }

    fn scan_branch<'a>(
        &'a self,
        query: &StorageBranchScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Entry>>> {
        self.delegate.scan_branch(query, context)
    }

    fn scan_branch_structure<'a>(
        &'a self,
        query: &StorageBranchScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<EntryStructure>>> {
        self.delegate.scan_branch_structure(query, context)
    }

    fn scan_entries<'a>(
        &'a self,
        query: &EntryScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Entry>>> {
        self.delegate.scan_entries(query, context)
    }

    fn scan_usage<'a>(
        &'a self,
        query: &UsageScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<UsageRow>>> {
        self.delegate.scan_usage(query, context)
    }

    fn get_stats<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<SessionStats>> {
        self.delegate.get_stats(context)
    }

    fn close<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<()>> {
        self.delegate.close(context)
    }
}

/// Upstream `CommitDiscarded` (`testing/gating-storage.ts`): thrown for
/// every commit rejected after simulated storage loss.
#[derive(Debug)]
pub struct CommitDiscarded(pub &'static str);

impl std::fmt::Display for CommitDiscarded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for CommitDiscarded {}

struct ParkedCommit {
    release_tx: tokio::sync::oneshot::Sender<()>,
    landing: Arc<tokio::sync::Notify>,
    dropped: Mutex<bool>,
}

/// Upstream `GatingStorage` (`testing/gating-storage.ts`): test-only storage
/// decorator that deterministically parks admitted commits.
pub struct GatingStorage {
    base: DecoratorStorage,
    state: Mutex<GatingState>,
}

struct GatingState {
    armed: bool,
    discarded: bool,
    queue: Vec<ParkedCommit>,
}

impl GatingStorage {
    /// Upstream `new GatingStorage(delegate)`.
    pub fn new(delegate: Arc<dyn Storage>) -> Arc<Self> {
        Arc::new(GatingStorage {
            base: DecoratorStorage { delegate },
            state: Mutex::new(GatingState {
                armed: false,
                discarded: false,
                queue: Vec::new(),
            }),
        })
    }

    /// Upstream `arm()`: fixture setup bypasses gating until armed.
    pub fn arm(&self) {
        self.state.lock().unwrap().armed = true;
    }

    /// Upstream `pending()`.
    pub fn pending(&self) -> usize {
        self.state.lock().unwrap().queue.len()
    }

    /// Upstream `discard()`: drop parked commits and permanently reject
    /// every later commit.
    pub fn discard(&self) {
        let mut state = self.state.lock().unwrap();
        if state.discarded {
            return;
        }
        state.discarded = true;
        for parked in state.queue.drain(..) {
            let mut dropped = parked.dropped.lock().unwrap();
            *dropped = true;
            parked.release_tx.send(()).ok();
        }
    }

    /// Upstream `next(count)`: release `count` commits in FIFO order and
    /// wait until each write lands.
    pub async fn next(&self, count: usize) -> anyhow::Result<()> {
        for _ in 0..count {
            let parked = {
                let mut state = self.state.lock().unwrap();
                if state.queue.is_empty() {
                    anyhow::bail!("No parked commit");
                }
                state.queue.remove(0)
            };
            parked.release_tx.send(()).ok();
            parked.landing.notified().await;
        }
        Ok(())
    }
}

impl Storage for GatingStorage {
    fn commit<'a>(
        &'a self,
        writes: Vec<Write>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<CommitResult>> {
        Box::pin(async move {
            let (armed, discarded) = {
                let state = self.state.lock().unwrap();
                (state.armed, state.discarded)
            };
            if discarded {
                anyhow::bail!("{}", CommitDiscarded("commit rejected: storage discarded"));
            }
            if !armed {
                return self.base.commit(writes, context).await;
            }
            let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
            let landing = Arc::new(tokio::sync::Notify::new());
            let parked = ParkedCommit {
                release_tx,
                landing: Arc::clone(&landing),
                dropped: Mutex::new(false),
            };
            self.state.lock().unwrap().queue.push(parked);
            release_rx
                .await
                .map_err(|_| anyhow::anyhow!("{}", CommitDiscarded("commit discarded")))?;
            {
                let state = self.state.lock().unwrap();
                if state.discarded {
                    anyhow::bail!("{}", CommitDiscarded("commit rejected: storage discarded"));
                }
            }
            let result = self.base.commit(writes, context).await;
            // Mark the landing for `next()` / drop paths.
            let landing_notify = {
                let _state = self.state.lock().unwrap();
                result.is_ok()
            };
            let _ = landing_notify;
            landing.notify_one();
            result
        })
    }

    fn get_entries<'a>(
        &'a self,
        ids: &[String],
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<std::collections::HashMap<String, Entry>>> {
        self.base.get_entries(ids, context)
    }

    fn get_value<'a>(
        &'a self,
        address: &ValueAddress,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<StoredValue>>> {
        self.base.get_value(address, context)
    }

    fn scan_values<'a>(
        &'a self,
        prefix: &ValueAddress,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<StoredValue>>> {
        self.base.scan_values(prefix, context)
    }

    fn read_list<'a>(
        &'a self,
        address: &ValueAddress,
        options: Option<&ListReadOptions>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<ListElement>>> {
        self.base.read_list(address, options, context)
    }

    fn scan_branch<'a>(
        &'a self,
        query: &StorageBranchScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Entry>>> {
        self.base.scan_branch(query, context)
    }

    fn scan_branch_structure<'a>(
        &'a self,
        query: &StorageBranchScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<EntryStructure>>> {
        self.base.scan_branch_structure(query, context)
    }

    fn scan_entries<'a>(
        &'a self,
        query: &EntryScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Entry>>> {
        self.base.scan_entries(query, context)
    }

    fn scan_usage<'a>(
        &'a self,
        query: &UsageScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<UsageRow>>> {
        self.base.scan_usage(query, context)
    }

    fn get_stats<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<SessionStats>> {
        self.base.get_stats(context)
    }

    fn close<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<()>> {
        self.base.close(context)
    }
}

/// Upstream `InstrumentedStorage` (`testing/instrumented-storage.ts`):
/// test-only transparent decorator that records commit admission.
pub struct InstrumentedStorage {
    base: DecoratorStorage,
    commit_attempts: Mutex<Vec<Vec<Write>>>,
}

impl InstrumentedStorage {
    /// Upstream `new InstrumentedStorage(delegate)`.
    pub fn new(delegate: Arc<dyn Storage>) -> Arc<Self> {
        Arc::new(InstrumentedStorage {
            base: DecoratorStorage { delegate },
            commit_attempts: Mutex::new(Vec::new()),
        })
    }

    /// Upstream `getCommitAttempts()`.
    pub fn get_commit_attempts(&self) -> Vec<Vec<Write>> {
        self.commit_attempts.lock().unwrap().clone()
    }

    /// Upstream `clearCommitAttempts()`.
    pub fn clear_commit_attempts(&self) {
        self.commit_attempts.lock().unwrap().clear();
    }
}

impl Storage for InstrumentedStorage {
    fn commit<'a>(
        &'a self,
        writes: Vec<Write>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<CommitResult>> {
        Box::pin(async move {
            self.commit_attempts.lock().unwrap().push(writes.clone());
            self.base.commit(writes, context).await
        })
    }

    fn get_entries<'a>(
        &'a self,
        ids: &[String],
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<std::collections::HashMap<String, Entry>>> {
        self.base.get_entries(ids, context)
    }

    fn get_value<'a>(
        &'a self,
        address: &ValueAddress,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<StoredValue>>> {
        self.base.get_value(address, context)
    }

    fn scan_values<'a>(
        &'a self,
        prefix: &ValueAddress,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<StoredValue>>> {
        self.base.scan_values(prefix, context)
    }

    fn read_list<'a>(
        &'a self,
        address: &ValueAddress,
        options: Option<&ListReadOptions>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<ListElement>>> {
        self.base.read_list(address, options, context)
    }

    fn scan_branch<'a>(
        &'a self,
        query: &StorageBranchScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Entry>>> {
        self.base.scan_branch(query, context)
    }

    fn scan_branch_structure<'a>(
        &'a self,
        query: &StorageBranchScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<EntryStructure>>> {
        self.base.scan_branch_structure(query, context)
    }

    fn scan_entries<'a>(
        &'a self,
        query: &EntryScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Entry>>> {
        self.base.scan_entries(query, context)
    }

    fn scan_usage<'a>(
        &'a self,
        query: &UsageScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<UsageRow>>> {
        self.base.scan_usage(query, context)
    }

    fn get_stats<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<SessionStats>> {
        self.base.get_stats(context)
    }

    fn close<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<()>> {
        self.base.close(context)
    }
}
