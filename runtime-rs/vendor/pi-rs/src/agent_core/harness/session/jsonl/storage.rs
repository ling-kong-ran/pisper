//! Port of `packages/agent/src/harness/session/jsonl/storage.ts` (267
//! lines): [`JsonlStorage`] — the file-backed format-4 [`Storage`] over an
//! injected [`FileSystem`], with torn-tail repair on open, one-line
//! transaction appends, atomic legacy v3 upgrade on first commit, and the
//! imported-usage view.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use futures::future::BoxFuture;

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::session::commit::{insert_usage, CommittedWrite};
use crate::agent_core::harness::session::jsonl::io::{
    file_value, parse_jsonl_transaction, publish_file_atomically, publish_jsonl, read_jsonl_header,
    serialize_jsonl_transaction,
};
use crate::agent_core::harness::session::jsonl::legacy_v3::LegacyV3Source;
use crate::agent_core::harness::session::jsonl::types::{
    JsonlStorageHeader, JsonlStorageOptions, JSONL_STORAGE_VERSION,
};
use crate::agent_core::harness::session::storage_state::InMemoryStorageState;
use crate::agent_core::harness::session::types::{
    CommitResult, Entry, EntryScan, EntryStructure, NewUsageRow, SessionStats, Storage,
    StorageBranchScan, UsageRow, UsageScan, Write,
};
use crate::agent_core::harness::session::values::{
    ListElement, ListReadOptions, StoredValue, ValueAddress,
};
use crate::ai::uuid;

/// Upstream lifecycle literals (`storage.ts:48`).
const OPEN: u8 = 0;
const CLOSING: u8 = 1;
const CLOSED: u8 = 2;

/// Upstream `splitCompleteLines(content)` (`storage.ts:30-35`).
fn split_complete_lines(content: &str) -> (Vec<&str>, bool) {
    if let Some(stripped) = content.strip_suffix('\n') {
        if stripped.is_empty() {
            return (Vec::new(), false);
        }
        return (stripped.split('\n').collect(), false);
    }
    match content.rfind('\n') {
        None => (Vec::new(), true),
        Some(last_newline) => (content[..last_newline].split('\n').collect(), true),
    }
}

/// Upstream `JsonlBacking` (`storage.ts:37`).
#[derive(Clone)]
enum JsonlBacking {
    V4,
    V3(Arc<LegacyV3Source>),
}

/// Upstream `JsonlStorage` (`storage.ts:40-267`): JSONL storage backed by an
/// injected filesystem capability.
pub struct JsonlStorage {
    file_system: Arc<dyn crate::agent_core::harness::types::FileSystem>,
    path: String,
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
    header: JsonlStorageHeader,
    /// Mutated by the one-time v3 upgrade; guarded by the commit lock.
    backing: tokio::sync::Mutex<JsonlBacking>,
    storage_state: tokio::sync::Mutex<InMemoryStorageState>,
    lifecycle: AtomicU8,
    close_gate: tokio::sync::OnceCell<()>,
}

impl std::fmt::Debug for JsonlStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonlStorage")
            .field("path", &self.path)
            .field("header", &self.header)
            .finish_non_exhaustive()
    }
}

impl JsonlStorage {
    /// Upstream `readonly header` (`storage.ts:44`).
    pub fn header(&self) -> &JsonlStorageHeader {
        &self.header
    }

    fn new(
        options: &JsonlStorageOptions,
        header: JsonlStorageHeader,
        backing: JsonlBacking,
    ) -> Self {
        JsonlStorage {
            file_system: Arc::clone(&options.file_system),
            path: options.path.clone(),
            now: options
                .now
                .clone()
                .unwrap_or_else(|| Arc::new(crate::ai::now_ms)),
            header,
            backing: tokio::sync::Mutex::new(backing),
            storage_state: tokio::sync::Mutex::new(InMemoryStorageState::new()),
            lifecycle: AtomicU8::new(OPEN),
            close_gate: tokio::sync::OnceCell::new(),
        }
    }

    /// Upstream `JsonlStorage.create` (`storage.ts:59-72`): publish the
    /// header and initial writes atomically.
    pub async fn create(
        options: JsonlStorageOptions,
        header: JsonlStorageHeader,
        initial_writes: Vec<Write>,
        context: Context,
    ) -> anyhow::Result<JsonlStorage> {
        let storage = JsonlStorage::new(&options, header, JsonlBacking::V4);
        let timestamp = (storage.now)();
        let prepared = storage
            .storage_state
            .lock()
            .await
            .prepare_commit(initial_writes, timestamp)?;
        let prepared_writes = prepared.writes.clone();
        publish_jsonl(
            options.file_system.as_ref(),
            &options.path,
            &storage.header,
            context.clone(),
            |append| async move {
                if !prepared_writes.is_empty() {
                    append.append(&prepared_writes).await?;
                }
                Ok(())
            },
        )
        .await?;
        storage
            .storage_state
            .lock()
            .await
            .apply_validated(prepared.writes);
        Ok(storage)
    }

    /// Upstream `JsonlStorage.open` (`storage.ts:74-83`).
    pub async fn open(
        options: JsonlStorageOptions,
        context: Context,
    ) -> anyhow::Result<JsonlStorage> {
        let reader = file_value(
            options
                .file_system
                .open_text_line_reader(&options.path, context.clone())
                .await,
            &format!("Failed to read JSONL storage {}", options.path),
        )?;
        let parsed = read_jsonl_header(reader.as_ref(), &options.path, context.clone()).await;
        reader.close(context.clone()).await;
        let parsed = parsed?;
        match parsed {
            crate::agent_core::harness::session::jsonl::codec::JsonlParsedSessionHeader::V3Legacy { header } => {
                JsonlStorage::open_legacy_v3(options, header, context).await
            }
            crate::agent_core::harness::session::jsonl::codec::JsonlParsedSessionHeader::V4 { header } => {
                JsonlStorage::open_v4(options, header, context).await
            }
        }
    }

    async fn open_v4(
        options: JsonlStorageOptions,
        header: JsonlStorageHeader,
        context: Context,
    ) -> anyhow::Result<JsonlStorage> {
        let content = file_value(
            options
                .file_system
                .read_text_file(&options.path, context.clone())
                .await,
            &format!("Failed to read JSONL storage {}", options.path),
        )?;
        let (lines, torn) = split_complete_lines(&content);
        if header.storage_version != JSONL_STORAGE_VERSION {
            anyhow::bail!(
                "Session {} uses unsupported storage version {}",
                header.id,
                header.storage_version
            );
        }
        let storage = JsonlStorage::new(&options, header, JsonlBacking::V4);
        for (index, line) in lines.iter().enumerate().skip(1) {
            let writes = match parse_jsonl_transaction(line) {
                Ok(writes) => writes,
                Err(error) => {
                    anyhow::bail!(
                        "Invalid JSONL storage {}: line {}: {error}",
                        options.path,
                        index + 1
                    );
                }
            };
            if let Err(error) = storage.replay_committed(writes).await {
                anyhow::bail!(
                    "Invalid JSONL storage {}: line {}: {error}",
                    options.path,
                    index + 1
                );
            }
        }
        if let Some(next_seq) = storage.header.next_seq {
            storage
                .storage_state
                .lock()
                .await
                .advance_next_seq(next_seq)?;
        }
        if torn {
            let joined = format!("{}\n", lines.join("\n"));
            publish_file_atomically(
                options.file_system.as_ref(),
                &options.path,
                context,
                |append| async move { append.append(&joined).await },
            )
            .await?;
        }
        Ok(storage)
    }

    async fn open_legacy_v3(
        options: JsonlStorageOptions,
        header: crate::agent_core::harness::session::jsonl::codec::LegacyV3SessionHeader,
        context: Context,
    ) -> anyhow::Result<JsonlStorage> {
        // The normalized v4 header is rebuilt from the source scan below.
        let _ = header;
        let source = LegacyV3Source::read(
            Arc::clone(&options.file_system),
            &options.path,
            context.clone(),
        )
        .await?;
        let source = Arc::new(source);
        let mut header = source.header.clone();
        header.next_seq = Some(source.next_seq);
        let storage = JsonlStorage::new(&options, header, JsonlBacking::V3(Arc::clone(&source)));
        let writes = source.writes(context.clone(), None).await?;
        for write in writes {
            storage.replay_committed(vec![write]).await?;
        }
        Ok(storage)
    }

    /// Upstream `replayCommitted` (`storage.ts:123-126`).
    async fn replay_committed(&self, writes: Vec<CommittedWrite>) -> anyhow::Result<()> {
        let mut state = self.storage_state.lock().await;
        state.validate_committed(&writes)?;
        state.apply_validated(writes);
        Ok(())
    }

    /// Upstream `withImportedUsage` (`storage.ts:240-242`).
    async fn with_imported_usage(&self, stats: SessionStats) -> SessionStats {
        let backing = self.backing.lock().await;
        match &*backing {
            JsonlBacking::V4 => stats,
            JsonlBacking::V3(source) => SessionStats {
                usage: source.imported_usage,
                ..stats
            },
        }
    }

    /// Upstream `isLegacyV3` (`storage.ts:244-246`).
    pub async fn is_legacy_v3(&self) -> bool {
        matches!(&*self.backing.lock().await, JsonlBacking::V3(_))
    }

    /// Upstream `captureForkNextSeq` (`storage.ts:249-257`): capture the
    /// first sequence a later source commit would use.
    pub async fn capture_fork_next_seq(&self) -> anyhow::Result<i64> {
        if self.lifecycle.load(Ordering::SeqCst) != OPEN {
            anyhow::bail!("JsonlStorage is closed");
        }
        Ok(self.storage_state.lock().await.get_next_seq())
    }

    /// Upstream `applyCommit` (`storage.ts:138-151`) — the caller holds the
    /// storage-state lock (the upstream commitQueue serialization).
    async fn apply_commit(
        &self,
        state: &mut InMemoryStorageState,
        writes: Vec<Write>,
        context: Context,
    ) -> anyhow::Result<CommitResult> {
        let backing_is_v3 = matches!(&*self.backing.lock().await, JsonlBacking::V3(_));
        if backing_is_v3 && !writes.is_empty() {
            let source = match &*self.backing.lock().await {
                JsonlBacking::V3(source) => Arc::clone(source),
                JsonlBacking::V4 => unreachable!("checked above"),
            };
            return self
                .upgrade_legacy_v3_to_v4(state, source, writes, context)
                .await;
        }
        let timestamp = (self.now)();
        let prepared = state.prepare_commit(writes, timestamp)?;
        if !prepared.writes.is_empty() {
            file_value(
                self.file_system
                    .append_file(
                        &self.path,
                        crate::agent_core::harness::types::FileContent::Text(format!(
                            "{}\n",
                            serialize_jsonl_transaction(&prepared.writes)?
                        )),
                        context.clone(),
                    )
                    .await,
                &format!("Failed to append JSONL storage {}", self.path),
            )?;
        }
        let stats = state.apply_validated(prepared.writes);
        let stats = self.with_imported_usage(stats).await;
        Ok(prepared.result.with_stats(stats))
    }

    /// Upstream `upgradeLegacyV3ToV4` (`storage.ts:154-189`): atomically
    /// upgrade the legacy backing and preserve the first caller write as a
    /// v4 transaction.
    async fn upgrade_legacy_v3_to_v4(
        &self,
        state: &mut InMemoryStorageState,
        source: Arc<LegacyV3Source>,
        caller_writes: Vec<Write>,
        context: Context,
    ) -> anyhow::Result<CommitResult> {
        let timestamp = (self.now)();
        let adjustment_id = uuid::uuid_v7_at(timestamp).expect("uuidv7 timestamp in range");
        let imported_usage = source.imported_usage;
        let mut staged = vec![insert_usage(NewUsageRow {
            id: adjustment_id,
            usage: imported_usage,
            entry_id: None,
            adjustment: true,
            details: Some(serde_json::json!({ "source": "v3-import" })),
        })];
        staged.extend(caller_writes);
        let prepared = state.prepare_commit(staged, timestamp)?;

        let next_seq = prepared.result.first_seq + prepared.writes.len() as i64;
        let mut upgraded_header = self.header.clone();
        upgraded_header.next_seq = Some(next_seq);
        publish_jsonl(
            self.file_system.as_ref(),
            &self.path,
            &upgraded_header,
            context.clone(),
            |append| {
                let source = Arc::clone(&source);
                let prepared_writes = prepared.writes.clone();
                async move {
                    let source_writes = source.writes(context.clone(), None).await?;
                    for write in source_writes {
                        append.append(std::slice::from_ref(&write)).await?;
                    }
                    append.append(&prepared_writes).await?;
                    Ok(())
                }
            },
        )
        .await?;

        let stats = state.apply_validated(prepared.writes);
        *self.backing.lock().await = JsonlBacking::V4;
        // The first sequence belongs to the internal usage adjustment;
        // return only caller-write sequences.
        Ok(CommitResult {
            first_seq: prepared.result.first_seq + 1,
            seqs: prepared.result.seqs.iter().skip(1).copied().collect(),
            timestamp: prepared.result.timestamp,
            stats,
        })
    }

    fn assert_open(&self) -> anyhow::Result<()> {
        if self.lifecycle.load(Ordering::SeqCst) != OPEN {
            anyhow::bail!("JsonlStorage is closed");
        }
        Ok(())
    }
}

impl Storage for JsonlStorage {
    fn commit<'a>(
        &'a self,
        writes: Vec<Write>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<CommitResult>> {
        Box::pin(async move {
            self.assert_open()?;
            // The commitQueue serialization: hold the storage-state lock
            // across validate + file append + apply, including the one-time
            // v3 upgrade.
            let mut state = self.storage_state.lock().await;
            self.apply_commit(&mut state, writes, context).await
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
            Ok(self.storage_state.lock().await.get_entries(&ids))
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
            Ok(self.storage_state.lock().await.get_value(&address))
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
            Ok(self.storage_state.lock().await.scan_values(&prefix))
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
            self.storage_state
                .lock()
                .await
                .read_list(&address, options.as_ref())
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
            self.storage_state.lock().await.scan_branch(&query)
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
            self.storage_state
                .lock()
                .await
                .scan_branch_structure(&query)
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
            Ok(self.storage_state.lock().await.scan_entries(&query))
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
            Ok(self.storage_state.lock().await.scan_usage(&query))
        })
    }

    fn get_stats<'a>(&'a self, _context: Context) -> BoxFuture<'a, anyhow::Result<SessionStats>> {
        Box::pin(async move {
            self.assert_open()?;
            let stats = self.storage_state.lock().await.get_stats();
            Ok(self.with_imported_usage(stats).await)
        })
    }

    fn close<'a>(&'a self, _context: Context) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            self.close_gate
                .get_or_init(|| async move {
                    self.lifecycle.store(CLOSING, Ordering::SeqCst);
                    let _drain = self.storage_state.lock().await;
                    self.lifecycle.store(CLOSED, Ordering::SeqCst);
                })
                .await;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests;
