//! Port of upstream `coding-agent/src/core/models-store.ts`: model-catalog
//! storage for the coding agent.
//!
//! [`InMemoryCodingAgentModelsStore`] and [`FileModelsStore`] implement the
//! pi-ai [`ModelsStore`] trait
//! ([`crate::ai::models::store::ModelsStore`], already ported).
//!
//! Seams / disclosed substitutions:
//! - `FileAuthStorageBackend` + `withLockAsync` (upstream
//!   `./auth-storage.ts`, not yet ported) are vendored here as
//!   [`FileAuthStorageBackend`] with exactly the `withLockAsync` behavior the
//!   store relies on: parent-dir/file creation (mode 0600 on creation), a
//!   `<path>.lock` directory as the lock unit (proper-lockfile's protocol),
//!   `ELOCKED` retry with the upstream backoff schedule (10 * 2^retry capped
//!   at 1s base delay, jittered, 30s deadline, abortable sleeps), read →
//!   mutate → write inside the lock, release afterwards. The proper-lockfile
//!   stale-lock `onCompromised` refresh thread is not reproduced (the lock
//!   has no background refresher, so the compromised path cannot fire).
//! - `Math.random()` jitter becomes a deterministic sawtooth increment
//!   (timing only; no observable ordering change).
//! - The module-global `sharedModelsFileReadState` single-slot cache keeps
//!   the upstream semantics: one process-wide shared read state, keyed to the
//!   path that first created it.
//! - Reload coalescing uses `futures::future::Shared` for the upstream shared
//!   promise; reader counting, settle-clear and the readers-to-zero
//!   cancel-and-clear follow the upstream sequence exactly.
//! - `StoredModels` preserves JSON key order (Vec of pairs) so file rewrites
//!   match `JSON.stringify(current, null, 2)` byte-for-byte (oracle-pinned
//!   against the real upstream store).

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use futures::future::{BoxFuture, Shared};
use futures::FutureExt;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::ai::models::store::{
    ModelsStore, ModelsStoreEntry, ModelsStoreError, ModelsStoreOperationOptions,
};
use crate::coding_agent::utils::abort::race_with_abort_signal;
use crate::coding_agent::utils::paths::{get_file_revision, normalize_path};
use crate::coding_agent::utils::text::strip_bom;

/// Upstream `StoredModels = Record<string, ModelsStoreEntry>`, in file
/// (document) order.
type StoredModels = Vec<(String, ModelsStoreEntry)>;

/// serde mirror of the pi-ai [`ModelsStoreEntry`] shape (the ai type itself
/// carries no serde derives and lives outside this slice): identical field
/// names (camelCase) and skip-when-absent optionality, so wire bytes are the
/// upstream TypeScript object shape.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelsStoreEntryWire {
    models: Vec<crate::ai::types::AnyModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_modified: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checked_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
}

impl From<&ModelsStoreEntry> for ModelsStoreEntryWire {
    fn from(entry: &ModelsStoreEntry) -> Self {
        Self {
            models: entry.models.clone(),
            last_modified: entry.last_modified,
            checked_at: entry.checked_at,
            etag: entry.etag.clone(),
        }
    }
}

impl From<ModelsStoreEntryWire> for ModelsStoreEntry {
    fn from(wire: ModelsStoreEntryWire) -> Self {
        ModelsStoreEntry {
            models: wire.models,
            last_modified: wire.last_modified,
            checked_at: wire.checked_at,
            etag: wire.etag,
        }
    }
}

fn parse_stored_models(content: &str) -> Result<StoredModels, ModelsStoreError> {
    let parsed: serde_json::Value = serde_json::from_str(strip_bom(content))
        .map_err(|error| ModelsStoreError::Storage(error.to_string()))?;
    let serde_json::Value::Object(map) = parsed else {
        // Upstream hands non-object JSON to property accessors and throws on
        // the way out; the port normalizes to a storage error.
        return Err(ModelsStoreError::Storage(
            "models store file is not an object".to_string(),
        ));
    };
    // JS object semantics: first key position wins, last value wins.
    let mut entries: StoredModels = Vec::new();
    for (key, value) in map {
        let wire: ModelsStoreEntryWire = serde_json::from_value(value)
            .map_err(|error| ModelsStoreError::Storage(error.to_string()))?;
        let entry: ModelsStoreEntry = wire.into();
        match entries.iter_mut().find(|(existing, _)| *existing == key) {
            Some(slot) => slot.1 = entry,
            None => entries.push((key, entry)),
        }
    }
    Ok(entries)
}

/// `JSON.stringify(current, null, 2)` for [`StoredModels`]: two-space indent,
/// document order. Entries serialize through the pi-ai serde impls (already
/// byte-matched to the upstream TypeScript wire format).
fn stringify_stored_models(models: &StoredModels) -> String {
    if models.is_empty() {
        return "{}".to_string();
    }
    // `JSON.stringify(value, null, 2)`: entries joined by ",\n" at the top
    // level, each entry re-indented by two spaces.
    let entry_blocks: Vec<String> = models
        .iter()
        .map(|(key, entry)| {
            let key_json = serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_string());
            let entry_json = serde_json::to_string_pretty(&ModelsStoreEntryWire::from(entry))
                .unwrap_or_else(|_| "{}".to_string());
            let mut block = format!("  {key_json}: ");
            for (line_index, line) in entry_json.lines().enumerate() {
                if line_index > 0 {
                    block.push_str("\n  ");
                }
                block.push_str(line);
            }
            block
        })
        .collect();
    format!("{{\n{}\n}}", entry_blocks.join(",\n"))
}

// ---------------------------------------------------------------------------
// InMemoryCodingAgentModelsStore
// ---------------------------------------------------------------------------

/// Upstream `InMemoryCodingAgentModelsStore`: a private in-memory catalog
/// (`structuredClone` on the read/write paths becomes ownership + clones).
#[derive(Default)]
pub struct InMemoryCodingAgentModelsStore {
    entries: RwLock<std::collections::HashMap<String, ModelsStoreEntry>>,
}

impl ModelsStore for InMemoryCodingAgentModelsStore {
    fn read<'a>(
        &'a self,
        provider_id: &'a str,
        options: &'a ModelsStoreOperationOptions,
    ) -> BoxFuture<'a, Result<Option<ModelsStoreEntry>, ModelsStoreError>> {
        Box::pin(async move {
            options.check()?;
            let entries = self
                .entries
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Ok(entries.get(provider_id).cloned())
        })
    }

    fn write<'a>(
        &'a self,
        provider_id: &'a str,
        entry: ModelsStoreEntry,
        options: &'a ModelsStoreOperationOptions,
    ) -> BoxFuture<'a, Result<(), ModelsStoreError>> {
        Box::pin(async move {
            options.check()?;
            self.entries
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(provider_id.to_string(), entry);
            Ok(())
        })
    }

    fn delete<'a>(
        &'a self,
        provider_id: &'a str,
        options: &'a ModelsStoreOperationOptions,
    ) -> BoxFuture<'a, Result<(), ModelsStoreError>> {
        Box::pin(async move {
            options.check()?;
            self.entries
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(provider_id);
            Ok(())
        })
    }
}

// ---------------------------------------------------------------------------
// FileAuthStorageBackend (vendored withLockAsync subset — see module docs)
// ---------------------------------------------------------------------------

/// Lock handle: upstream's `release()` closure. Dropping does not release —
/// call [`LockHandle::release`] explicitly (or use [`LockedGuard`]).
pub struct LockHandle {
    lock_path: String,
    released: bool,
}

impl LockHandle {
    /// proper-lockfile `release()`: remove the lock directory; errors are
    /// ignored (upstream ignores unlock errors when compromised).
    pub fn release(mut self) {
        self.released = true;
        let _ = std::fs::remove_dir(&self.lock_path);
    }
}

/// Lock acquisition counter (upstream tests spy on `proper-lockfile.lock`
/// call counts; disclosed test seam).
pub static LOCK_CALLS: AtomicUsize = AtomicUsize::new(0);

/// Serializes every test that asserts on the process-global [`LOCK_CALLS`]
/// counter (parallel suites observing the same spy would cross-count
/// otherwise). Test-only; lives beside the counter it guards.
#[cfg(test)]
pub(crate) static LOCK_SPY: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn io_error_is_locked(error: &std::io::Error) -> bool {
    matches!(error.kind(), std::io::ErrorKind::AlreadyExists)
}

/// Upstream `FileAuthStorageBackend` withLockAsync plumbing.
pub struct FileAuthStorageBackend {
    storage_path: String,
}

impl FileAuthStorageBackend {
    pub fn new(storage_path: String) -> Self {
        Self { storage_path }
    }

    fn lock_path(&self) -> String {
        format!("{}.lock", self.storage_path)
    }

    fn ensure_parent_dir(&self) {
        if let Some(parent) = std::path::Path::new(&self.storage_path).parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
    }

    fn ensure_file_exists(&self) -> Result<(), ModelsStoreError> {
        if !std::path::Path::new(&self.storage_path).exists() {
            write_storage_file(&self.storage_path, "{}")?;
        }
        Ok(())
    }

    /// Upstream `acquireLockAsync`: loop `create_dir` on `<path>.lock` with
    /// the upstream backoff schedule and abortable sleeps.
    fn acquire_lock_async(
        &self,
        signal: Option<CancellationToken>,
    ) -> BoxFuture<'_, Result<LockHandle, ModelsStoreError>> {
        Box::pin(async move {
            let stale_ms: u64 = 30_000;
            let max_delay_ms: u64 = 2_000;
            let deadline = std::time::Instant::now() + Duration::from_millis(stale_ms);
            let lock_path = self.lock_path();
            let mut retry: u32 = 0;
            let mut jitter_step: u64 = 0;
            let cancelled = |signal: &Option<CancellationToken>| {
                signal.as_ref().is_some_and(CancellationToken::is_cancelled)
            };
            loop {
                if cancelled(&signal) {
                    return Err(ModelsStoreError::Cancelled);
                }
                LOCK_CALLS.fetch_add(1, Ordering::SeqCst);
                match std::fs::create_dir(&lock_path) {
                    Ok(()) => {
                        let handle = LockHandle {
                            lock_path,
                            released: false,
                        };
                        // `if (signal?.aborted) { await release(); signal.throwIfAborted(); }`
                        if cancelled(&signal) {
                            handle.release();
                            return Err(ModelsStoreError::Cancelled);
                        }
                        return Ok(handle);
                    }
                    Err(error) if io_error_is_locked(&error) => {
                        if cancelled(&signal) {
                            return Err(ModelsStoreError::Cancelled);
                        }
                        let remaining =
                            deadline.saturating_duration_since(std::time::Instant::now());
                        if remaining.is_zero() {
                            return Err(ModelsStoreError::Storage(format!(
                                "ELOCKED: resource is locked: {lock_path}"
                            )));
                        }
                        // baseDelayMs = min(10 * 2**retry, maxDelayMs / 2)
                        let base_delay_ms = 10u64
                            .saturating_mul(1u64 << retry.min(20))
                            .min(max_delay_ms / 2);
                        retry += 1;
                        // Deterministic stand-in for the (1 + Math.random())
                        // jitter: a bounded +0..+100% sawtooth (timing only).
                        jitter_step = (jitter_step + 37) % 101;
                        let delay_ms = base_delay_ms
                            .saturating_mul(100 + jitter_step)
                            .saturating_div(100)
                            .min(remaining.as_millis() as u64);
                        let sleep = tokio::time::sleep(Duration::from_millis(delay_ms));
                        match signal.as_ref() {
                            Some(signal) => tokio::select! {
                                () = sleep => continue,
                                _ = signal.cancelled() => {
                                    return Err(ModelsStoreError::Cancelled);
                                }
                            },
                            None => {
                                sleep.await;
                                continue;
                            }
                        }
                    }
                    Err(error) => {
                        return Err(ModelsStoreError::Storage(error.to_string()));
                    }
                }
            }
        })
    }

    /// Upstream `withLockAsync(fn, options)`: create dirs/file, take the
    /// lock, read the current content, run `fn`, optionally write `next`,
    /// release afterwards. The callback mirrors the upstream `LockResult`:
    /// `(result, next?)` with `next` written under the lock; an `Err`
    /// rejects before anything is written.
    pub async fn with_lock_async<T, F>(
        &self,
        f: F,
        options: &ModelsStoreOperationOptions,
    ) -> Result<T, ModelsStoreError>
    where
        F: FnOnce(
            Option<String>,
        ) -> BoxFuture<'static, Result<(T, Option<String>), ModelsStoreError>>,
        T: Send + 'static,
    {
        options.check()?;
        self.ensure_parent_dir();
        self.ensure_file_exists()?;

        let mut release: Option<LockHandle> = None;
        let result = async {
            let handle = self.acquire_lock_async(options.signal.clone()).await?;
            release = Some(handle);
            options.check()?;
            let current = std::path::Path::new(&self.storage_path)
                .exists()
                .then(|| std::fs::read_to_string(&self.storage_path).ok())
                .flatten();
            let (result, next) = f(current).await?;
            options.check()?;
            if let Some(next) = next {
                write_storage_file(&self.storage_path, &next)?;
            }
            Ok(result)
        }
        .await;

        // upstream `finally { await release() }` — release errors ignored.
        if let Some(handle) = release.take() {
            handle.release();
        }
        result
    }
}

/// Upstream `writeFileSync(path, next, { mode: 0o600 })`: the mode applies on
/// creation only.
fn write_storage_file(path: &str, content: &str) -> Result<(), ModelsStoreError> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|error| ModelsStoreError::Storage(error.to_string()))?;
        file.write_all(content.as_bytes())
            .map_err(|error| ModelsStoreError::Storage(error.to_string()))
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, content).map_err(|error| ModelsStoreError::Storage(error.to_string()))
    }
}

// ---------------------------------------------------------------------------
// FileModelsStore
// ---------------------------------------------------------------------------

type SharedReload =
    Shared<Pin<Box<dyn Future<Output = Result<StoredModels, ModelsStoreError>> + Send>>>;

struct ModelsFileReload {
    token: CancellationToken,
    shared: SharedReload,
    readers: AtomicUsize,
}

#[derive(Default)]
struct ModelsFileReadState {
    data: StoredModels,
    revision: Option<String>,
    reload: Option<Arc<ModelsFileReload>>,
}

fn same_reload(a: &Option<Arc<ModelsFileReload>>, b: &Arc<ModelsFileReload>) -> bool {
    a.as_ref().is_some_and(|existing| Arc::ptr_eq(existing, b))
}

static SHARED_MODELS_FILE_READ_STATE: Mutex<Option<(String, Arc<Mutex<ModelsFileReadState>>)>> =
    Mutex::new(None);

/// Test-only: clear the process-global shared read state so a test can pin
/// which path seeds it (upstream tests rely on construction order within one
/// process; cargo runs this crate's tests in parallel).
#[cfg(test)]
pub fn reset_shared_read_state_for_tests() {
    *SHARED_MODELS_FILE_READ_STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

/// Locked JSON-backed storage for dynamically refreshed provider catalogs
/// (upstream `FileModelsStore`).
pub struct FileModelsStore {
    storage: FileAuthStorageBackend,
    path: String,
    read_state: Arc<Mutex<ModelsFileReadState>>,
}

impl FileModelsStore {
    /// Upstream `new FileModelsStore(path)` with an explicit path.
    pub fn new(path: impl Into<String>) -> Self {
        Self::build(path.into())
    }

    /// Upstream default: `join(getAgentDir(), "models-store.json")`.
    pub fn with_default_path() -> Self {
        Self::build(super::path_join(
            &super::get_agent_dir(),
            "models-store.json",
        ))
    }

    fn build(path: String) -> Self {
        let path = normalize_path(&path).unwrap_or(path);
        let storage = FileAuthStorageBackend::new(path.clone());
        let mut shared_guard = SHARED_MODELS_FILE_READ_STATE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let read_state = match shared_guard.as_ref() {
            Some((shared_path, read_state)) if shared_path == &path => Arc::clone(read_state),
            _ => {
                // Upstream seeds the shared slot exactly once (first path
                // wins); later distinct paths get private states.
                let read_state = Arc::new(Mutex::new(ModelsFileReadState::default()));
                if shared_guard.is_none() {
                    *shared_guard = Some((path.clone(), Arc::clone(&read_state)));
                }
                read_state
            }
        };
        drop(shared_guard);
        Self {
            storage,
            path,
            read_state,
        }
    }

    /// Upstream `updateReadState`.
    fn update_read_state(&self, data: StoredModels, revision: Option<String>) {
        let mut state = self
            .read_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.data = data;
        state.revision = revision;
    }

    /// Upstream `reloadFromStorage`: parse + update the shared read state
    /// inside the storage lock, then resolve with the data.
    fn reload_from_storage(
        &self,
        token: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<StoredModels, ModelsStoreError>> + Send>> {
        let storage_path = self.path.clone();
        let read_state = Arc::clone(&self.read_state);
        Box::pin(async move {
            struct LockedParse {
                parsed: Result<StoredModels, ModelsStoreError>,
                revision: Option<String>,
            }
            let backend = FileAuthStorageBackend::new(storage_path.clone());
            let locked: LockedParse = backend
                .with_lock_async(
                    |content| {
                        let storage_path = storage_path.clone();
                        Box::pin(async move {
                            let parsed = match content {
                                Some(content) => parse_stored_models(&content),
                                None => Ok(Vec::new()),
                            };
                            let revision = get_file_revision(&storage_path);
                            Ok((LockedParse { parsed, revision }, None))
                        })
                            as BoxFuture<
                                'static,
                                Result<(LockedParse, Option<String>), ModelsStoreError>,
                            >
                    },
                    &ModelsStoreOperationOptions::new(token),
                )
                .await?;
            let data = locked.parsed?;
            {
                // `this.updateReadState(readState, data, revision)` inside the
                // lock callback.
                let mut state = read_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state.data = data.clone();
                state.revision = locked.revision;
            }
            Ok(data)
        })
    }

    /// Upstream `readLatest`: serve from the revision cache, else coalesce
    /// concurrent readers onto one reload promise.
    async fn read_latest(
        &self,
        options: &ModelsStoreOperationOptions,
    ) -> Result<StoredModels, ModelsStoreError> {
        options.check()?;
        let revision = get_file_revision(&self.path);
        {
            let state = self
                .read_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if revision.is_some() && state.revision == revision {
                return Ok(state.data.clone());
            }
        }
        let reload = {
            let mut state = self
                .read_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.reload.is_none() {
                let token = CancellationToken::new();
                let shared = self.reload_from_storage(token.clone()).shared();
                let reload = Arc::new(ModelsFileReload {
                    token,
                    shared: shared.clone(),
                    readers: AtomicUsize::new(0),
                });
                // `void reload.promise.then(() => { if (readState.reload ===
                // reload) readState.reload = undefined; }, …)`
                let cleanup_reload = Arc::clone(&reload);
                let cleanup_state = Arc::clone(&self.read_state);
                tokio::spawn(async move {
                    let _ = shared.await;
                    let mut state = cleanup_state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if same_reload(&state.reload, &cleanup_reload) {
                        state.reload = None;
                    }
                });
                state.reload = Some(Arc::clone(&reload));
            }
            let reload = Arc::clone(state.reload.as_ref().expect("just installed"));
            reload.readers.fetch_add(1, Ordering::SeqCst);
            reload
        };

        let operation = reload.shared.clone();
        let result = race_with_abort_signal(operation, options.signal.as_ref()).await;

        // `finally { reload.readers--; if (readers === 0 && same) { clear +
        // abort; } }`
        let readers = reload.readers.fetch_sub(1, Ordering::SeqCst) - 1;
        if readers == 0 {
            let mut state = self
                .read_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if same_reload(&state.reload, &reload) {
                state.reload = None;
                drop(state);
                reload.token.cancel();
            }
        }

        match result {
            Ok(Ok(data)) => Ok(data),
            Ok(Err(error)) => Err(error),
            // `raceWithAbortSignal` rejects with the fixed abort error.
            Err(_abort) => Err(ModelsStoreError::Cancelled),
        }
    }
}

impl ModelsStore for FileModelsStore {
    fn read<'a>(
        &'a self,
        provider_id: &'a str,
        options: &'a ModelsStoreOperationOptions,
    ) -> BoxFuture<'a, Result<Option<ModelsStoreEntry>, ModelsStoreError>> {
        Box::pin(async move {
            let latest = self.read_latest(options).await?;
            options.check()?;
            Ok(latest
                .into_iter()
                .find(|(id, _)| id == provider_id)
                .map(|(_, entry)| entry))
        })
    }

    fn write<'a>(
        &'a self,
        provider_id: &'a str,
        entry: ModelsStoreEntry,
        options: &'a ModelsStoreOperationOptions,
    ) -> BoxFuture<'a, Result<(), ModelsStoreError>> {
        Box::pin(async move {
            let provider_id = provider_id.to_string();
            // The lock callback carries the resulting map back as its result
            // (upstream assigns the outer `latest` from inside the callback).
            let current: StoredModels = self
                .storage
                .with_lock_async(
                    |content| {
                        let provider_id = provider_id.clone();
                        let mut entry = entry;
                        Box::pin(async move {
                            let mut current = match content {
                                Some(content) => parse_stored_models(&content)?,
                                None => Vec::new(),
                            };
                            // JS `current[providerId] = entry`: update in
                            // place or append.
                            match current.iter_mut().find(|(id, _)| *id == provider_id) {
                                Some(slot) => slot.1 = std::mem::take(&mut entry),
                                None => current.push((provider_id, entry)),
                            }
                            let next = stringify_stored_models(&current);
                            Ok((current.clone(), Some(next)))
                        })
                            as BoxFuture<
                                'static,
                                Result<(StoredModels, Option<String>), ModelsStoreError>,
                            >
                    },
                    options,
                )
                .await?;
            // `if (latest) this.updateReadState(this.readState, latest);`
            self.update_read_state(current, None);
            Ok(())
        })
    }

    fn delete<'a>(
        &'a self,
        provider_id: &'a str,
        options: &'a ModelsStoreOperationOptions,
    ) -> BoxFuture<'a, Result<(), ModelsStoreError>> {
        Box::pin(async move {
            let provider_id = provider_id.to_string();
            let current: StoredModels = self
                .storage
                .with_lock_async(
                    |content| {
                        let provider_id = provider_id.clone();
                        Box::pin(async move {
                            let mut current = match content {
                                Some(content) => parse_stored_models(&content)?,
                                None => Vec::new(),
                            };
                            current.retain(|(id, _)| *id != provider_id);
                            let next = stringify_stored_models(&current);
                            Ok((current.clone(), Some(next)))
                        })
                            as BoxFuture<
                                'static,
                                Result<(StoredModels, Option<String>), ModelsStoreError>,
                            >
                    },
                    options,
                )
                .await?;
            self.update_read_state(current, None);
            Ok(())
        })
    }
}

#[cfg(test)]
#[allow(
    clippy::await_holding_lock,
    // Deliberate: the test module holds the process-global LOCK_CALLS spy
    // across awaits to serialize parallel store tests against the counter.
)]
#[path = "models_store_tests.rs"]
mod tests;
