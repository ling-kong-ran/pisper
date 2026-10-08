//! Port of upstream `coding-agent/src/core/auth-storage.ts`: the
//! CredentialStore implementation backed by `auth.json` (provider auth
//! orchestration belongs to ModelRuntime and the pi-ai Models, as upstream).
//!
//! Surfaces: the [`AuthStorageBackend`] storage contract ([`AuthStorageBackend::File`]
//! reusing the `withLockAsync` file-lock subset vendored in
//! [`super::models_store::FileAuthStorageBackend`] for the async path plus a
//! local sync-lock path; [`AuthStorageBackend::InMemory`] porting the
//! promise-chain serialization), [`AuthStorage`] (the locked JSON store
//! implementing [`CredentialStore`]), [`ReadOnlyAuthStorage`] (the validating
//! read-only store), and [`read_stored_credential`].
//!
//! Seams / disclosed substitutions:
//! - The async `<path>.lock` directory protocol, creation mode 0600, and the
//!   `ELOCKED` backoff schedule are **reused** from the vendored
//!   `models_store::FileAuthStorageBackend` instead of re-implemented; the
//!   `onCompromised` refresh thread is not reproduced there (no background
//!   refresher can compromise the lock) and the jitter is a deterministic
//!   sawtooth stand-in for `Math.random()` — both disclosures carry over.
//! - The **sync** `withLock` (upstream `acquireLockSyncWithRetry`: at most 10
//!   attempts with a 20ms synchronous busy-wait between `ELOCKED` results)
//!   is implemented here; it shares the same `<path>.lock` directory
//!   protocol. Constructor `reload()`s use this path, which is why async-lock
//!   spies never observe them (upstream spies `lockfile.lock` the same way).
//! - `auth.json` document key order is preserved through
//!   [`super::model_config::OrderedValue`] parsing, so rewrites match
//!   `JSON.stringify(merged, null, 2)` byte-for-byte (oracle-pinned). Inside
//!   one credential, field order follows the ai port's canonical declaration
//!   order (`type, key, env` / `type, refresh, access, expires`); upstream
//!   preserves the modify callback's JS object literal order, and the
//!   canonical order is what upstream's own saveAuth writes.
//! - Upstream `read()` hands out shared references into the read-state cache
//!   (OAuth entries) or shallow copies (resolved api-key entries), so caller
//!   mutations can leak into the cache (the oracle pins the leak); the port
//!   returns owned deep clones. No caller relies on the leak.
//! - Upstream tolerates arbitrary non-credential entries in `AuthStorage`
//!   (only `ReadOnlyAuthStorage` validates); the port surfaces a
//!   [`AuthError::Storage`] when an entry does not fit the `Credential` wire
//!   format. Read-only validation runs on the raw JSON fields first, with the
//!   upstream fixed error texts.
//! - Cancellation is `AuthOperationOptions`' `CancellationToken`; the fixed
//!   abort error maps to [`AuthError::Cancelled`]. A cancelled waiter's
//!   future is dropped at the await point instead of being detached
//!   (crate-wide `raceWithAbortSignal` divergence, same no-write guarantee);
//!   the in-memory async chain serializes through one global mutex, like
//!   upstream's single `asyncChain` promise.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use futures::future::{BoxFuture, Shared};
use futures::FutureExt;
use tokio_util::sync::CancellationToken;

use crate::ai::auth::credential_store::{CredentialStore, ModifyCallback};
use crate::ai::auth::types::{AuthError, AuthOperationOptions, Credential, CredentialInfo};
use crate::ai::models::store::{ModelsStoreError, ModelsStoreOperationOptions};
use crate::coding_agent::core::model_config::OrderedValue;
use crate::coding_agent::core::models_store::FileAuthStorageBackend;
use crate::coding_agent::core::path_join;
use crate::coding_agent::core::resolve_config_value::{
    is_command_config_value, resolve_config_value,
};
use crate::coding_agent::utils::abort::race_with_abort_signal;
use crate::coding_agent::utils::paths::{get_file_revision, normalize_path};
use crate::coding_agent::utils::text::strip_bom;

/// Upstream `AuthStorageData = Record<string, Credential>`, in file
/// (document) order.
pub type AuthStorageData = Vec<(String, Credential)>;

/// Upstream default `auth.json` location: `join(getAgentDir(), "auth.json")`.
pub fn default_auth_path() -> String {
    path_join(&super::get_agent_dir(), "auth.json")
}

// ---------------------------------------------------------------------------
// Parsing / serialization (document order)
// ---------------------------------------------------------------------------

/// Upstream `parseStorageData`: parse through [`OrderedValue`] so provider
/// insertion order survives, then convert each entry through the ai
/// `Credential` serde impl (wire-compatible).
fn parse_auth_data(content: Option<&str>) -> Result<AuthStorageData, AuthError> {
    let Some(content) = content else {
        // Upstream `if (!content) return {}` (empty string included).
        return Ok(Vec::new());
    };
    let stripped = strip_bom(content);
    let ordered: OrderedValue =
        serde_json::from_str(stripped).map_err(|error| AuthError::Storage(error.to_string()))?;
    // Upstream casts `JSON.parse` output to the record type without
    // validation; non-object documents behave as empty data through the
    // property accesses the store performs (disclosed normalization).
    let OrderedValue::Object(entries) = ordered else {
        return Ok(Vec::new());
    };
    let mut data: AuthStorageData = Vec::with_capacity(entries.len());
    for (key, value) in entries {
        let credential: Credential = serde_json::from_value(value.to_serde())
            .map_err(|error| AuthError::Storage(error.to_string()))?;
        // JS object semantics: first key position wins, last value wins.
        match data.iter_mut().find(|(existing, _)| *existing == key) {
            Some(slot) => slot.1 = credential,
            None => data.push((key, credential)),
        }
    }
    Ok(data)
}

/// `JSON.stringify(data, null, 2)` for [`AuthStorageData`]: two-space indent,
/// document order, empty document as `{}`.
fn stringify_auth_data(data: &AuthStorageData) -> String {
    if data.is_empty() {
        return "{}".to_string();
    }
    let entry_blocks: Vec<String> = data
        .iter()
        .map(|(key, credential)| {
            let key_json = serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_string());
            let credential_json =
                serde_json::to_string_pretty(credential).unwrap_or_else(|_| "{}".to_string());
            let mut block = format!("  {key_json}: ");
            for (line_index, line) in credential_json.lines().enumerate() {
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

fn upsert_credential(data: &mut AuthStorageData, provider_id: &str, credential: Credential) {
    match data
        .iter_mut()
        .find(|(existing, _)| existing == provider_id)
    {
        Some(slot) => slot.1 = credential,
        None => data.push((provider_id.to_string(), credential)),
    }
}

// ---------------------------------------------------------------------------
// Sync file-lock plumbing (upstream FileAuthStorageBackend.withLock)
// ---------------------------------------------------------------------------

/// Upstream `AUTH_FILE_WRITE_OPTIONS` (`{ encoding: "utf-8", mode: 0o600 }`):
/// the mode applies on creation only.
fn write_auth_file(path: &str, content: &str) -> Result<(), AuthError> {
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
            .map_err(|error| AuthError::Storage(error.to_string()))?;
        file.write_all(content.as_bytes())
            .map_err(|error| AuthError::Storage(error.to_string()))
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, content).map_err(|error| AuthError::Storage(error.to_string()))
    }
}

fn ensure_parent_dir(path: &str) {
    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
}

/// Sync lock handle: removing the `<path>.lock` directory releases.
struct SyncLockHandle {
    lock_path: String,
}

impl SyncLockHandle {
    fn release(self) {
        let _ = std::fs::remove_dir(&self.lock_path);
    }
}

/// Upstream `acquireLockSyncWithRetry`: `proper-lockfile.lockSync` on
/// `<path>.lock`, at most 10 attempts, an `ELOCKED` result busy-waits 20ms
/// synchronously before retrying, any other error (or the final attempt)
/// throws.
fn acquire_lock_sync_with_retry(path: &str) -> Result<SyncLockHandle, AuthError> {
    const MAX_ATTEMPTS: usize = 10;
    const DELAY_MS: u64 = 20;
    let lock_path = format!("{path}.lock");
    for attempt in 1..=MAX_ATTEMPTS {
        match std::fs::create_dir(&lock_path) {
            Ok(()) => return Ok(SyncLockHandle { lock_path }),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if attempt == MAX_ATTEMPTS {
                    return Err(AuthError::Storage(format!(
                        "ELOCKED: resource is locked: {lock_path}"
                    )));
                }
                let start = Instant::now();
                while start.elapsed() < Duration::from_millis(DELAY_MS) {
                    // Sleep synchronously to avoid changing callers to async.
                    std::hint::spin_loop();
                }
            }
            Err(error) => return Err(AuthError::Storage(error.to_string())),
        }
    }
    Err(AuthError::Storage(
        "Failed to acquire auth storage lock".to_string(),
    ))
}

/// Upstream sync `withLock` over a file: ensure the parent dir and the `{}`
/// file, take the sync lock, read, run `fn`, optionally write, release.
fn file_with_lock_sync<T>(
    path: &str,
    f: impl FnOnce(Option<String>) -> (T, Option<String>),
) -> Result<T, AuthError> {
    ensure_parent_dir(path);
    if !std::path::Path::new(path).exists() {
        write_auth_file(path, "{}")?;
    }

    let mut release: Option<SyncLockHandle> = None;
    let result = (|| {
        release = Some(acquire_lock_sync_with_retry(path)?);
        let current = std::path::Path::new(path)
            .exists()
            .then(|| std::fs::read_to_string(path).ok())
            .flatten();
        let (result, next) = f(current);
        if let Some(next) = next {
            write_auth_file(path, &next)?;
        }
        Ok(result)
    })();

    // upstream `finally { release() }`
    if let Some(handle) = release {
        handle.release();
    }
    result
}

// ---------------------------------------------------------------------------
// InMemoryAuthStorageBackend + the AuthStorageBackend surface
// ---------------------------------------------------------------------------

/// Upstream `InMemoryAuthStorageBackend`: a private in-memory document with
/// one global serialization chain for async operations. The fields are Arcs
/// so a signalled operation can be detached onto the runtime (upstream's
/// promise keeps running after `raceWithAbortSignal` rejects its caller).
#[derive(Default)]
pub struct InMemoryAuthStorageBackend {
    value: Arc<std::sync::Mutex<Option<String>>>,
    chain: Arc<tokio::sync::Mutex<()>>,
}

impl InMemoryAuthStorageBackend {
    fn with_lock_sync<T>(
        &self,
        f: impl FnOnce(Option<String>) -> (T, Option<String>),
    ) -> Result<T, AuthError> {
        let mut value = self
            .value
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (result, next) = f(value.clone());
        if let Some(next) = next {
            *value = Some(next);
        }
        Ok(result)
    }
}

/// Upstream `AuthStorageBackend`: the storage contract [`AuthStorage`] runs
/// on (sync `withLock` for reloads, async `withLockAsync` for mutations).
pub enum AuthStorageBackend {
    /// `FileAuthStorageBackend`: the async path reuses the vendored
    /// `models_store` lock subset; the path is carried here so the sync
    /// variant and reload plumbing can share it.
    File {
        path: String,
    },
    InMemory(InMemoryAuthStorageBackend),
}

impl Clone for AuthStorageBackend {
    fn clone(&self) -> Self {
        match self {
            AuthStorageBackend::File { path } => AuthStorageBackend::File { path: path.clone() },
            AuthStorageBackend::InMemory(backend) => {
                AuthStorageBackend::InMemory(InMemoryAuthStorageBackend {
                    value: Arc::clone(&backend.value),
                    chain: Arc::clone(&backend.chain),
                })
            }
        }
    }
}

fn map_store_error(error: ModelsStoreError) -> AuthError {
    match error {
        ModelsStoreError::Cancelled => AuthError::Cancelled,
        ModelsStoreError::Storage(message) => AuthError::Storage(message),
    }
}

fn map_auth_error(error: AuthError) -> ModelsStoreError {
    match error {
        AuthError::Cancelled => ModelsStoreError::Cancelled,
        AuthError::Storage(message)
        | AuthError::Operation(message)
        // Ripple of the v1.0.0 AuthError::AddressInUse variant (OAuth
        // callback-port-in-use detection); same storage-error channel.
        | AuthError::AddressInUse(message) => ModelsStoreError::Storage(message),
        AuthError::Models(models_error) => ModelsStoreError::Storage(models_error.to_string()),
    }
}

impl AuthStorageBackend {
    /// Upstream sync `withLock`: run `fn(current)` under the storage lock,
    /// writing `next` when the callback returns one.
    pub fn with_lock_sync<T>(
        &self,
        f: impl FnOnce(Option<String>) -> (T, Option<String>),
    ) -> Result<T, AuthError> {
        match self {
            AuthStorageBackend::File { path } => file_with_lock_sync(path, f),
            AuthStorageBackend::InMemory(backend) => backend.with_lock_sync(f),
        }
    }

    /// Upstream `withLockAsync(fn, options)`: the callback sees the current
    /// document and returns `(result, next?)`; `next` is written under the
    /// lock; an `Err` rejects before anything is written.
    pub async fn with_lock_async<T, F>(
        &self,
        f: F,
        options: &AuthOperationOptions,
    ) -> Result<T, AuthError>
    where
        F: FnOnce(Option<String>) -> BoxFuture<'static, Result<(T, Option<String>), AuthError>>
            + Send
            + 'static,
        T: Send + 'static,
    {
        match self {
            AuthStorageBackend::File { path } => {
                let backend = FileAuthStorageBackend::new(path.clone());
                let store_options = ModelsStoreOperationOptions {
                    signal: options.signal.clone(),
                };
                backend
                    .with_lock_async(
                        move |content| {
                            let fut = f(content);
                            Box::pin(async move { fut.await.map_err(map_auth_error) })
                                as BoxFuture<'static, Result<(T, Option<String>), ModelsStoreError>>
                        },
                        &store_options,
                    )
                    .await
                    .map_err(map_store_error)
            }
            AuthStorageBackend::InMemory(backend) => {
                // `const previous = this.asyncChain; const operation =
                // (async () => { await previous…; throwIfAborted; … })();
                // this.asyncChain = operation.catch(() => {}); return
                // raceWithAbortSignal(operation, options?.signal);` — the
                // caller races the operation and rejects early on abort
                // while the detached operation keeps running.
                let value = Arc::clone(&backend.value);
                let chain = Arc::clone(&backend.chain);
                let owned_options = options.clone();
                let signal = owned_options.signal.clone();
                let operation = async move {
                    let _chain = chain.lock().await;
                    owned_options.check()?;
                    let current = value
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone();
                    let (result, next) = f(current).await?;
                    owned_options.check()?;
                    if let Some(next) = next {
                        *value
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(next);
                    }
                    Ok(result)
                };
                match signal {
                    None => operation.await,
                    Some(signal) => {
                        let (tx, rx) = tokio::sync::oneshot::channel::<Result<T, AuthError>>();
                        tokio::spawn(async move {
                            let _ = tx.send(operation.await);
                        });
                        let raced: Result<
                            Result<T, AuthError>,
                            crate::coding_agent::utils::abort::AbortError,
                        > = race_with_abort_signal(
                            async move {
                                match rx.await {
                                    Ok(result) => result,
                                    Err(_) => Err(AuthError::Cancelled),
                                }
                            },
                            Some(&signal),
                        )
                        .await;
                        match raced {
                            Ok(Ok(result)) => Ok(result),
                            Ok(Err(error)) => Err(error),
                            Err(_abort) => Err(AuthError::Cancelled),
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// AuthStorage
// ---------------------------------------------------------------------------

type SharedReload =
    Shared<Pin<Box<dyn Future<Output = Result<AuthStorageData, AuthError>> + Send>>>;

struct AuthFileReload {
    token: CancellationToken,
    shared: SharedReload,
    readers: std::sync::atomic::AtomicUsize,
}

#[derive(Default)]
struct AuthFileReadState {
    data: AuthStorageData,
    revision: Option<String>,
    reload: Option<Arc<AuthFileReload>>,
}

fn same_reload(a: &Option<Arc<AuthFileReload>>, b: &Arc<AuthFileReload>) -> bool {
    a.as_ref().is_some_and(|existing| Arc::ptr_eq(existing, b))
}

static SHARED_AUTH_FILE_READ_STATE: RwLock<Option<(String, Arc<Mutex<AuthFileReadState>>)>> =
    RwLock::new(None);

/// Test-only: clear the process-global shared read-state slot (upstream seeds
/// it from construction order within one process; cargo runs this crate's
/// tests in parallel).
#[cfg(test)]
pub fn reset_shared_read_state_for_tests() {
    *SHARED_AUTH_FILE_READ_STATE
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

/// Upstream `AuthStorage`: the CredentialStore backed by a JSON file (or an
/// in-memory backend). The process-global single-slot read-state cache is
/// keyed to the first file path that created it, exactly like upstream.
pub struct AuthStorage {
    storage: AuthStorageBackend,
    auth_path: Option<String>,
    read_state: Arc<Mutex<AuthFileReadState>>,
}

impl AuthStorage {
    /// Upstream `AuthStorage.create(authPath)` (default `auth.json` in the
    /// agent dir — see [`default_auth_path`]/[`Self::create_default`]).
    pub fn create(auth_path: &str) -> Self {
        let normalized = normalize_path(auth_path).unwrap_or_else(|_| auth_path.to_string());
        Self::build(
            AuthStorageBackend::File {
                path: normalized.clone(),
            },
            Some(normalized),
        )
    }

    /// Upstream default-path overload of [`Self::create`].
    pub fn create_default() -> Self {
        Self::create(&default_auth_path())
    }

    /// Upstream `AuthStorage.fromStorage(storage)`.
    pub fn from_storage(storage: AuthStorageBackend) -> Self {
        Self::build(storage, None)
    }

    /// Upstream `AuthStorage.inMemory(data)`.
    pub fn in_memory(data: AuthStorageData) -> Self {
        let storage = AuthStorageBackend::InMemory(InMemoryAuthStorageBackend::default());
        let document = stringify_auth_data(&data);
        storage
            .with_lock_sync(|_| ((), Some(document)))
            .expect("in-memory seed cannot fail");
        Self::from_storage(storage)
    }

    fn build(storage: AuthStorageBackend, auth_path: Option<String>) -> Self {
        let shared_guard = SHARED_AUTH_FILE_READ_STATE
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let shared = shared_guard.as_ref();
        let read_state = match (shared, &auth_path) {
            (Some((shared_path, read_state)), Some(path)) if shared_path == path => {
                Arc::clone(read_state)
            }
            _ => Arc::new(Mutex::new(AuthFileReadState {
                data: Vec::new(),
                revision: None,
                reload: None,
            })),
        };
        drop(shared_guard);
        if auth_path.is_some() {
            let mut shared = SHARED_AUTH_FILE_READ_STATE
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if shared.is_none() {
                let path = auth_path.clone().expect("checked above");
                *shared = Some((path, Arc::clone(&read_state)));
            }
        }

        let storage = Self {
            storage,
            auth_path,
            read_state,
        };
        let needs_reload = if let Some(path) = &storage.auth_path {
            let revision = get_file_revision(path);
            let cached = {
                let state = storage
                    .read_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                revision.is_some() && revision == state.revision
            };
            !cached
        } else {
            true
        };
        if needs_reload {
            storage.reload();
        }
        storage
    }

    fn update_read_state(&self, data: AuthStorageData, revision: Option<String>) {
        let mut state = self
            .read_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.data = data;
        state.revision = revision;
    }

    fn snapshot_data(&self) -> AuthStorageData {
        self.read_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .data
            .clone()
    }

    /// Upstream `reload()`: re-read through the **sync** lock; failures
    /// preserve the last valid in-memory snapshot.
    pub fn reload(&self) {
        let outcome = self.storage.with_lock_sync(|current| {
            let revision = self
                .auth_path
                .as_ref()
                .and_then(|path| get_file_revision(path));
            ((current, revision), None)
        });
        if let Ok((content, revision)) = outcome {
            if let Ok(data) = parse_auth_data(content.as_deref()) {
                self.update_read_state(data, revision);
            }
        }
    }

    /// Upstream `reloadFromStorageAsync`: parse + update the read state
    /// inside the storage lock, then resolve with the data.
    fn reload_from_storage(
        &self,
        options: &AuthOperationOptions,
    ) -> Pin<Box<dyn Future<Output = Result<AuthStorageData, AuthError>> + Send>> {
        let read_state = Arc::clone(&self.read_state);
        let auth_path = self.auth_path.clone();
        let storage = self.storage.clone();
        let options = options.clone();
        Box::pin(async move {
            // The revision rides along inside the lock callback (upstream
            // assigns the outer `revision` there).
            #[allow(dead_code)]
            struct Locked {
                parsed: Result<AuthStorageData, AuthError>,
                revision: Option<String>,
            }
            let locked: Locked = storage
                .with_lock_async(
                    move |content| {
                        let read_state = Arc::clone(&read_state);
                        let auth_path = auth_path.clone();
                        Box::pin(async move {
                            let parsed = parse_auth_data(content.as_deref());
                            let revision = auth_path.and_then(|path| get_file_revision(&path));
                            if let Ok(data) = &parsed {
                                let mut state = read_state
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                state.data = data.clone();
                                state.revision = revision.clone();
                            }
                            Ok((Locked { parsed, revision }, None))
                        })
                            as BoxFuture<'static, Result<(Locked, Option<String>), AuthError>>
                    },
                    &options,
                )
                .await?;
            locked.parsed
        })
    }

    /// Upstream `readLatestData`: serve from the revision cache, else
    /// coalesce concurrent readers onto one shared reload promise.
    async fn read_latest_data(
        &self,
        options: &AuthOperationOptions,
    ) -> Result<AuthStorageData, AuthError> {
        options.check()?;
        if self.auth_path.is_none() {
            let reload = self.reload_from_storage(options);
            return match options.signal {
                Some(_) => reload.await,
                None => Ok(reload.await.unwrap_or_else(|_| self.snapshot_data())),
            };
        }

        let auth_path = self.auth_path.clone().expect("checked above");
        let revision = get_file_revision(&auth_path);
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
                let shared = self
                    .reload_from_storage(&AuthOperationOptions::new(token.clone()))
                    .shared();
                let reload = Arc::new(AuthFileReload {
                    token,
                    shared: shared.clone(),
                    readers: std::sync::atomic::AtomicUsize::new(0),
                });
                // `void reload.promise.then(() => { if (readState.reload ===
                // reload) readState.reload = undefined; }, () => { … })`
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
            reload
                .readers
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            reload
        };

        let result = race_with_abort_signal(reload.shared.clone(), options.signal.as_ref()).await;

        // `finally { reload.readers--; if (readers === 0 && same) { clear +
        // abort; } }`
        let readers = reload
            .readers
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst)
            - 1;
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
            // `options?.signal ? await result : await result.catch(() =>
            // this.readState.data)` — signal-less readers fall back to the
            // last snapshot when the coalesced reload fails.
            Ok(Err(error)) if options.signal.is_none() => {
                let _ = error;
                Ok(self.snapshot_data())
            }
            Ok(Err(error)) => Err(error),
            Err(_abort) => Err(AuthError::Cancelled),
        }
    }
}

impl CredentialStore for AuthStorage {
    fn read<'a>(
        &'a self,
        provider_id: &'a str,
        options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<Option<Credential>, AuthError>> {
        Box::pin(async move {
            let data = self.read_latest_data(options).await?;
            options.check()?;
            let credential = data
                .into_iter()
                .find(|(id, _)| id == provider_id)
                .map(|(_, credential)| credential);
            let Some(credential) = credential else {
                return Ok(None);
            };
            // `if (credential?.type !== "api_key") return credential;
            //  if (credential.key === undefined) return credential;
            //  return { ...credential, key: resolveConfigValue(…) }`
            match credential {
                Credential::ApiKey(mut api_key) => {
                    let Some(key) = api_key.key.clone() else {
                        return Ok(Some(Credential::ApiKey(api_key)));
                    };
                    api_key.key = resolve_config_value(&key, api_key.env.as_ref());
                    Ok(Some(Credential::ApiKey(api_key)))
                }
                resolved => Ok(Some(resolved)),
            }
        })
    }

    fn list<'a>(
        &'a self,
        options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<Vec<CredentialInfo>, AuthError>> {
        Box::pin(async move {
            let data = self.read_latest_data(options).await?;
            options.check()?;
            Ok(data
                .into_iter()
                .map(|(provider_id, credential)| CredentialInfo {
                    provider_id,
                    r#type: credential.auth_type(),
                })
                .collect())
        })
    }

    fn modify<'a>(
        &'a self,
        provider_id: &'a str,
        f: ModifyCallback,
        options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<Option<Credential>, AuthError>> {
        let provider_id = provider_id.to_string();
        Box::pin(async move {
            struct Mutation {
                result: Option<Credential>,
                latest: AuthStorageData,
                revision: Option<String>,
            }
            // The lock callback carries `(result, latestData, revision)` back
            // (upstream assigns the outer `latestData`/`revision` from inside
            // the callback); a callback failure rejects before
            // `updateReadState` runs.
            let auth_path = self.auth_path.clone();
            let mutation: Mutation = self
                .storage
                .with_lock_async(
                    move |content| {
                        let mut f = Some(f);
                        Box::pin(async move {
                            let current = parse_auth_data(content.as_deref())?;
                            let current_credential = current
                                .iter()
                                .find(|(id, _)| *id == provider_id)
                                .map(|(_, credential)| credential.clone());
                            let next_credential = f.take().expect("lock callback runs once")(
                                current_credential.clone(),
                            )
                            .await?;
                            let Some(next_credential) = next_credential else {
                                // `latestData = currentData; revision =
                                // this.authPath ? getFileRevision(…) : undefined`
                                let revision = auth_path.and_then(|path| get_file_revision(&path));
                                return Ok((
                                    Mutation {
                                        result: current_credential,
                                        latest: current.clone(),
                                        revision,
                                    },
                                    None,
                                ));
                            };
                            let mut merged = current;
                            upsert_credential(&mut merged, &provider_id, next_credential.clone());
                            Ok((
                                Mutation {
                                    result: Some(next_credential),
                                    latest: merged.clone(),
                                    revision: None,
                                },
                                Some(stringify_auth_data(&merged)),
                            ))
                        })
                            as BoxFuture<'static, Result<(Mutation, Option<String>), AuthError>>
                    },
                    options,
                )
                .await?;
            self.update_read_state(mutation.latest, mutation.revision);
            Ok(mutation.result)
        })
    }

    fn delete<'a>(
        &'a self,
        provider_id: &'a str,
        options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<(), AuthError>> {
        let provider_id = provider_id.to_string();
        Box::pin(async move {
            let latest: AuthStorageData = self
                .storage
                .with_lock_async(
                    move |content| {
                        let provider_id = provider_id.clone();
                        Box::pin(async move {
                            let mut current = parse_auth_data(content.as_deref())?;
                            current.retain(|(id, _)| id.as_str() != provider_id);
                            let next = stringify_auth_data(&current);
                            Ok((current.clone(), Some(next)))
                        })
                            as BoxFuture<
                                'static,
                                Result<(AuthStorageData, Option<String>), AuthError>,
                            >
                    },
                    options,
                )
                .await?;
            // Upstream `updateReadState(latestData)` — revision cleared.
            self.update_read_state(latest, None);
            Ok(())
        })
    }
}

// ---------------------------------------------------------------------------
// ReadOnlyAuthStorage
// ---------------------------------------------------------------------------

/// Upstream `ReadOnlyAuthStorage`: validating, memoized reads over auth.json.
pub struct ReadOnlyAuthStorage {
    auth_path: String,
    data: std::sync::Mutex<Option<AuthStorageData>>,
}

impl ReadOnlyAuthStorage {
    pub fn new(auth_path: &str) -> Self {
        Self {
            auth_path: normalize_path(auth_path).unwrap_or_else(|_| auth_path.to_string()),
            data: std::sync::Mutex::new(None),
        }
    }

    pub fn new_default() -> Self {
        Self::new(&default_auth_path())
    }

    /// Upstream `load()`: ENOENT memoizes `{}`; read/parse failures wrap as
    /// `Failed to read auth.json: …`; shape violations carry the fixed
    /// validation texts (oracle-pinned). Failures are not memoized.
    fn load(&self) -> Result<AuthStorageData, AuthError> {
        {
            let data = self
                .data
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(data) = data.as_ref() {
                return Ok(data.clone());
            }
        }

        let content = match std::fs::read_to_string(&self.auth_path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let empty: AuthStorageData = Vec::new();
                *self
                    .data
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(empty.clone());
                return Ok(empty);
            }
            Err(error) => {
                return Err(AuthError::Storage(format!(
                    "Failed to read auth.json: {error}"
                )));
            }
        };

        let document: OrderedValue = serde_json::from_str(strip_bom(&content))
            .map_err(|error| AuthError::Storage(format!("Failed to read auth.json: {error}")))?;
        // `typeof parsed !== "object" || parsed === null ||
        // Array.isArray(parsed)`
        let OrderedValue::Object(entries) = document else {
            return Err(AuthError::Storage(
                "Invalid auth.json: expected an object".to_string(),
            ));
        };

        let mut data: AuthStorageData = Vec::with_capacity(entries.len());
        for (provider_id, value) in entries {
            let invalid = || {
                AuthError::Storage(format!(
                    "Invalid auth.json credential for provider \"{provider_id}\""
                ))
            };
            let OrderedValue::Object(fields) = &value else {
                return Err(invalid());
            };
            let field = |name: &str| {
                fields
                    .iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| value)
            };
            // Upstream validates the raw shapes before accepting:
            // - api_key: `key` absent or string; `env` absent or a non-array
            //   object of string values.
            // - oauth: `access`/`refresh` strings, finite `expires` number.
            match field("type").and_then(OrderedValue::as_str) {
                Some("api_key") => {
                    let valid_key = matches!(field("key"), None | Some(OrderedValue::String(_)));
                    let valid_env = match field("env") {
                        None => true,
                        Some(OrderedValue::Object(env_entries)) => env_entries
                            .iter()
                            .all(|(_, value)| matches!(value, OrderedValue::String(_))),
                        _ => false,
                    };
                    if !(valid_key && valid_env) {
                        return Err(invalid());
                    }
                }
                Some("oauth") => {
                    let valid = matches!(field("access"), Some(OrderedValue::String(_)))
                        && matches!(field("refresh"), Some(OrderedValue::String(_)))
                        && matches!(
                            field("expires"),
                            Some(OrderedValue::Number(number))
                                if number.as_f64().is_some_and(f64::is_finite)
                        );
                    if !valid {
                        return Err(invalid());
                    }
                }
                _ => return Err(invalid()),
            }
            let credential: Credential =
                serde_json::from_value(value.to_serde()).map_err(|_| invalid())?;
            data.push((provider_id, credential));
        }
        // First key position wins, last value wins (JS object semantics).
        let mut deduped: AuthStorageData = Vec::with_capacity(data.len());
        for (key, credential) in data {
            match deduped.iter_mut().find(|(existing, _)| *existing == key) {
                Some(slot) => slot.1 = credential,
                None => deduped.push((key, credential)),
            }
        }
        *self
            .data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(deduped.clone());
        Ok(deduped)
    }
}

impl CredentialStore for ReadOnlyAuthStorage {
    fn read<'a>(
        &'a self,
        provider_id: &'a str,
        options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<Option<Credential>, AuthError>> {
        Box::pin(async move {
            options.check()?;
            let data = self.load()?;
            options.check()?;
            let credential = data
                .into_iter()
                .find(|(id, _)| id == provider_id)
                .map(|(_, credential)| credential);
            let Some(credential) = credential else {
                return Ok(None);
            };
            // `if (credential.type !== "api_key" || !credential.key ||
            // isCommandConfigValue(credential.key)) return
            // structuredClone(credential);` — an empty-string key is falsy,
            // so it skips resolution too.
            match &credential {
                Credential::ApiKey(api_key) => {
                    let resolve_needed = api_key
                        .key
                        .as_deref()
                        .is_some_and(|key| !key.is_empty() && !is_command_config_value(key));
                    if !resolve_needed {
                        return Ok(Some(credential));
                    }
                    let key = api_key.key.as_deref().expect("checked above");
                    let mut resolved = api_key.clone();
                    resolved.key = resolve_config_value(key, api_key.env.as_ref());
                    Ok(Some(Credential::ApiKey(resolved)))
                }
                _ => Ok(Some(credential)),
            }
        })
    }

    fn list<'a>(
        &'a self,
        options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<Vec<CredentialInfo>, AuthError>> {
        Box::pin(async move {
            options.check()?;
            let data = self.load()?;
            options.check()?;
            Ok(data
                .into_iter()
                .map(|(provider_id, credential)| CredentialInfo {
                    provider_id,
                    r#type: credential.auth_type(),
                })
                .collect())
        })
    }

    fn modify<'a>(
        &'a self,
        _provider_id: &'a str,
        _f: ModifyCallback,
        _options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<Option<Credential>, AuthError>> {
        Box::pin(async {
            Err(AuthError::Operation(
                "Read-only credential storage cannot modify auth.json".to_string(),
            ))
        })
    }

    fn delete<'a>(
        &'a self,
        _provider_id: &'a str,
        _options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<(), AuthError>> {
        Box::pin(async {
            Err(AuthError::Operation(
                "Read-only credential storage cannot modify auth.json".to_string(),
            ))
        })
    }
}

// ---------------------------------------------------------------------------
// readStoredCredential
// ---------------------------------------------------------------------------

/// Upstream `readStoredCredential`: one-off synchronous read of a stored
/// credential from an auth.json file, without instantiating a store or
/// resolving configured key values; any failure yields `None`.
pub fn read_stored_credential(provider_id: &str, auth_path: &str) -> Option<Credential> {
    let normalized = normalize_path(auth_path).unwrap_or_else(|_| auth_path.to_string());
    let content = std::fs::read_to_string(&normalized).ok()?;
    let ordered: OrderedValue = serde_json::from_str(strip_bom(&content)).ok()?;
    let OrderedValue::Object(entries) = ordered else {
        return None;
    };
    let (_, value) = entries.into_iter().find(|(key, _)| key == provider_id)?;
    serde_json::from_value(value.to_serde()).ok()
}

#[cfg(test)]
pub(crate) fn parse_auth_data_for_tests(source: &str) -> Result<AuthStorageData, AuthError> {
    parse_auth_data(Some(source))
}

#[cfg(test)]
pub(crate) fn stringify_auth_data_for_tests(data: &AuthStorageData) -> String {
    stringify_auth_data(data)
}

#[cfg(test)]
#[allow(
    clippy::await_holding_lock,
    // Deliberate: the tests serialize process-global state (the shared
    // read-state slot, the LOCK_CALLS spy) with a std Mutex taken across the
    // whole test body; the guards never participate in a runtime-internal
    // dependency, so the awaits cannot deadlock on them (the models_store
    // tests use the same convention).
)]
#[path = "auth_storage_tests.rs"]
mod tests;
