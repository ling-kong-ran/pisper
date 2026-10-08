//! Port of `packages/agent/src/harness/session/jsonl/repo.ts` (389 lines):
//! [`JsonlSessionRepo`] — the file-backed format-4 session repository
//! lifecycle (cwd-scoped discovery, exclusive open handles, atomic create
//! publication, and the open/closed/legacy-v3 fork dispatch).
//!
//! Disclosed substitutions:
//! - Upstream `JsonlSessionRepo implements
//!   SessionRepo<JsonlSessionMetadata, JsonlSessionCreateOptions, ...>` — a
//!   generic-interface instantiation whose create/fork signatures carry the
//!   jsonl-specific option types; the port keeps those signatures as
//!   inherent methods instead of the flattened non-generic
//!   [`crate::agent_core::harness::session::types::SessionRepo`] trait,
//!   whose shapes cannot express the cwd-carrying options.
//! - `localeCompare` ordering approximates as case-insensitive character
//!   comparison (list sorting tie-break only).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::session::jsonl::codec::{
    metadata_from_header, parse_jsonl_session_header,
};
use crate::agent_core::harness::session::jsonl::fork::{
    run_jsonl_fork, JsonlForkInput, JsonlForkOptions, JsonlForkSourceMetadata,
};
use crate::agent_core::harness::session::jsonl::io::file_value;
use crate::agent_core::harness::session::jsonl::iso8601::format_iso8601_utc;
use crate::agent_core::harness::session::jsonl::legacy_v3::LegacyV3Source;
use crate::agent_core::harness::session::jsonl::storage::JsonlStorage;
use crate::agent_core::harness::session::jsonl::types::{
    JsonlSessionCreateOptions, JsonlSessionListOptions, JsonlSessionRepoOptions,
    JsonlStorageHeader, JsonlStorageOptions, JSONL_STORAGE_VERSION,
};
use crate::agent_core::harness::session::session::{
    StorageBackedSession, StorageBackedSessionOptions,
};
use crate::agent_core::harness::session::types::{ForkOptions, SessionMetadata, Storage};
use crate::agent_core::harness::types::{CreateDirOptions, FileInfo, FileSystem, RemoveOptions};
use crate::ai::uuid;

/// Upstream `sessionDirectoryName(cwd)` (`repo.ts:36-38`).
fn session_directory_name(cwd: &str) -> String {
    let stripped = cwd.trim_start_matches(['/', '\\']);
    let encoded: String = stripped
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c == ':' {
                '-'
            } else {
                c
            }
        })
        .collect();
    format!("--{encoded}--")
}

/// Upstream `sessionFileName(createdAt, id)` (`repo.ts:40-43`).
fn session_file_name(created_at: i64, id: &str) -> String {
    let timestamp = format_iso8601_utc(created_at).replace([':', '.'], "-");
    format!("{timestamp}_{}.jsonl", encode_uri_component(id))
}

/// Upstream `encodeURIComponent` (the JS global): unreserved characters
/// `A-Z a-z 0-9 - _ . ! ~ * ' ( )` pass through; everything else is
/// percent-encoded UTF-8.
fn encode_uri_component(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(byte as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Upstream `localeCompare` approximation: case-insensitive character
/// comparison, falling back to exact comparison.
fn locale_compare(left: &str, right: &str) -> std::cmp::Ordering {
    let lower = left
        .chars()
        .flat_map(char::to_lowercase)
        .cmp(right.chars().flat_map(char::to_lowercase));
    if lower == std::cmp::Ordering::Equal {
        left.cmp(right)
    } else {
        lower
    }
}

/// The mutable repo state (open sessions and pending creates).
#[derive(Default)]
struct RepoState {
    open_sessions: HashMap<String, Arc<JsonlStorage>>,
    pending_creates: HashSet<String>,
}

/// Upstream `JsonlSessionRepo` (`repo.ts:46-389`): file-backed format-4
/// session repository lifecycle.
pub struct JsonlSessionRepo {
    file_system: Arc<dyn FileSystem>,
    sessions_root_input: String,
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
    /// Shared so the session `on_close` hooks can deregister their handles.
    state: Arc<std::sync::Mutex<RepoState>>,
    closed: AtomicBool,
}

impl std::fmt::Debug for JsonlSessionRepo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonlSessionRepo")
            .field("sessions_root", &self.sessions_root_input)
            .finish_non_exhaustive()
    }
}

impl JsonlSessionRepo {
    /// Upstream `new JsonlSessionRepo(options)` (`repo.ts:57-61`).
    pub fn new(options: JsonlSessionRepoOptions) -> Self {
        JsonlSessionRepo {
            file_system: Arc::clone(&options.file_system),
            sessions_root_input: options.sessions_root,
            now: options.now.unwrap_or_else(|| Arc::new(crate::ai::now_ms)),
            state: Arc::new(std::sync::Mutex::new(RepoState::default())),
            closed: AtomicBool::new(false),
        }
    }

    fn assert_open(&self) -> anyhow::Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            anyhow::bail!("JsonlSessionRepo is closed");
        }
        Ok(())
    }

    fn session_key(cwd: &str, id: &str) -> String {
        format!("{cwd}\0{id}")
    }

    async fn root(&self, context: Context) -> anyhow::Result<String> {
        file_value(
            self.file_system
                .absolute_path(&self.sessions_root_input, context)
                .await,
            &format!(
                "Failed to resolve sessions root {}",
                self.sessions_root_input
            ),
        )
    }

    /// Upstream `create` (`repo.ts:63-93`).
    pub async fn create(
        &self,
        options: JsonlSessionCreateOptions,
        context: Context,
    ) -> anyhow::Result<Arc<StorageBackedSession>> {
        self.assert_open()?;
        let created_at = (self.now)();
        let destination_id = match &options.id {
            Some(id) => id.clone(),
            None => uuid::uuid_v7_at(created_at).expect("uuidv7 timestamp in range"),
        };
        let id = destination_id;
        let cwd = file_value(
            self.file_system
                .absolute_path(&options.cwd, context.clone())
                .await,
            &format!("Failed to resolve session cwd {}", options.cwd),
        )?;
        let key = Self::session_key(&cwd, &id);
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.open_sessions.contains_key(&key) || state.pending_creates.contains(&key) {
                anyhow::bail!("Session already exists: {id}");
            }
            state.pending_creates.insert(key.clone());
        }
        let mut path: Option<String> = None;
        let mut storage: Option<Arc<JsonlStorage>> = None;
        let result = async {
            let resolved_path = self
                .resolve_new_session_path(&cwd, created_at, &id, context.clone())
                .await?;
            path = Some(resolved_path.clone());
            let header = JsonlStorageHeader {
                parent_session_id: options.parent_session_id.clone(),
                ..JsonlStorageHeader::new(
                    id.clone(),
                    JSONL_STORAGE_VERSION,
                    created_at,
                    cwd.clone(),
                )
            };
            let created = Arc::new(
                JsonlStorage::create(
                    JsonlStorageOptions {
                        file_system: Arc::clone(&self.file_system),
                        path: resolved_path.clone(),
                        now: Some(Arc::clone(&self.now)),
                    },
                    header.clone(),
                    Vec::new(),
                    context.clone(),
                )
                .await?,
            );
            storage = Some(Arc::clone(&created));
            let info: FileInfo = file_value(
                self.file_system
                    .file_info(&resolved_path, context.clone())
                    .await,
                &format!("Failed to read session {resolved_path}"),
            )?;
            let metadata = metadata_from_header(&header, &resolved_path, info.mtime_ms);
            self.publish_open_session(metadata, created, &key, context.clone())
                .await
        }
        .await;
        if let Err(error) = &result {
            if let Some(storage) = &storage {
                let _ = storage.close(context.clone()).await;
            }
            if let Some(path) = &path {
                let _ = self
                    .file_system
                    .remove(
                        path,
                        Some(&RemoveOptions {
                            recursive: None,
                            force: Some(true),
                        }),
                        context.clone(),
                    )
                    .await;
            }
            let _ = error;
        }
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending_creates
            .remove(&key);
        result
    }

    /// Upstream `open` (`repo.ts:95-107`).
    pub async fn open(
        &self,
        metadata: &SessionMetadata,
        context: Context,
    ) -> anyhow::Result<Arc<StorageBackedSession>> {
        self.assert_open()?;
        let key = Self::session_key(metadata.cwd.as_deref().unwrap_or_default(), &metadata.id);
        {
            let state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.open_sessions.contains_key(&key) {
                anyhow::bail!("Session is already open: {}", metadata.id);
            }
        }
        let storage = self.load_storage(metadata, context.clone()).await?;
        self.publish_open_session(metadata.clone(), storage, &key, context)
            .await
    }

    /// Upstream `list` (`repo.ts:109-129`).
    pub async fn list(
        &self,
        options: Option<&JsonlSessionListOptions>,
        context: Context,
    ) -> anyhow::Result<Vec<SessionMetadata>> {
        let fallback = JsonlSessionListOptions::default();
        let options = options.unwrap_or(&fallback);
        self.assert_open()?;
        let cwd = match &options.cwd {
            None => None,
            Some(cwd) => Some(file_value(
                self.file_system.absolute_path(cwd, context.clone()).await,
                &format!("Failed to resolve session cwd {cwd}"),
            )?),
        };
        let root = self.root(context.clone()).await?;
        if !file_value(
            self.file_system.exists(&root, context.clone()).await,
            &format!("Failed to check sessions root {root}"),
        )? {
            return Ok(Vec::new());
        }
        let directories = match &cwd {
            None => self.session_directories(&root, context.clone()).await?,
            Some(cwd) => vec![self.session_directory(cwd, context.clone()).await?],
        };
        let mut metadata = Vec::new();
        for directory in &directories {
            metadata.extend(
                self.list_directory(directory, cwd.as_deref(), context.clone())
                    .await?,
            );
        }
        metadata.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| locale_compare(&left.id, &right.id))
                .then_with(|| {
                    locale_compare(
                        left.cwd.as_deref().unwrap_or_default(),
                        right.cwd.as_deref().unwrap_or_default(),
                    )
                })
        });
        Ok(metadata)
    }

    /// Upstream `delete` (`repo.ts:131-144`).
    pub async fn delete(&self, metadata: &SessionMetadata, context: Context) -> anyhow::Result<()> {
        self.assert_open()?;
        let key = Self::session_key(metadata.cwd.as_deref().unwrap_or_default(), &metadata.id);
        if self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .open_sessions
            .contains_key(&key)
        {
            anyhow::bail!("Session is open: {}", metadata.id);
        }
        let path = metadata.path.as_deref().unwrap_or_default();
        if !file_value(
            self.file_system.exists(path, context.clone()).await,
            &format!("Failed to check session {path}"),
        )? {
            anyhow::bail!("Session file does not exist: {path}");
        }
        file_value(
            self.file_system.remove(path, None, context).await,
            &format!("Failed to delete session {path}"),
        )
    }

    /// Upstream `fork` (`repo.ts:146-196`).
    pub async fn fork(
        &self,
        source: &SessionMetadata,
        options: &ForkOptions,
        context: Context,
    ) -> anyhow::Result<Arc<StorageBackedSession>> {
        self.assert_open()?;
        let created_at = (self.now)();
        let cwd = source.cwd.clone().unwrap_or_default();
        let id = match options {
            ForkOptions::Branch { id, .. } | ForkOptions::Tree { id } => match id {
                Some(id) => id.clone(),
                None => uuid::uuid_v7_at(created_at).expect("uuidv7 timestamp in range"),
            },
        };
        let destination_key = Self::session_key(&cwd, &id);
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.open_sessions.contains_key(&destination_key)
                || state.pending_creates.contains(&destination_key)
            {
                anyhow::bail!("Session already exists: {id}");
            }
            state.pending_creates.insert(destination_key.clone());
        }
        let source_key = Self::session_key(&source.cwd.clone().unwrap_or_default(), &source.id);
        let source_storage = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .open_sessions
            .get(&source_key)
            .map(Arc::clone);
        let mut path: Option<String> = None;
        let mut storage: Option<Arc<JsonlStorage>> = None;
        let result = async {
            let input = self
                .resolve_fork_input(source, source_storage.as_ref(), context.clone())
                .await?;
            let resolved_path = self
                .resolve_new_session_path(&cwd, created_at, &id, context.clone())
                .await?;
            path = Some(resolved_path.clone());
            let header = JsonlStorageHeader {
                parent_session_id: Some(source.id.clone()),
                ..JsonlStorageHeader::new(
                    id.clone(),
                    JSONL_STORAGE_VERSION,
                    created_at,
                    cwd.clone(),
                )
            };
            run_jsonl_fork(
                JsonlForkOptions {
                    input,
                    file_system: Arc::clone(&self.file_system),
                    destination_path: resolved_path.clone(),
                    destination_header: header.clone(),
                    fork: options.clone(),
                },
                context.clone(),
            )
            .await?;
            let opened = Arc::new(
                JsonlStorage::open(
                    JsonlStorageOptions {
                        file_system: Arc::clone(&self.file_system),
                        path: resolved_path.clone(),
                        now: Some(Arc::clone(&self.now)),
                    },
                    context.clone(),
                )
                .await?,
            );
            storage = Some(Arc::clone(&opened));
            let info: FileInfo = file_value(
                self.file_system
                    .file_info(&resolved_path, context.clone())
                    .await,
                &format!("Failed to read session {resolved_path}"),
            )?;
            let metadata = metadata_from_header(&header, &resolved_path, info.mtime_ms);
            self.publish_open_session(metadata, opened, &destination_key, context.clone())
                .await
        }
        .await;
        if let Err(error) = &result {
            if let Some(storage) = &storage {
                let _ = storage.close(context.clone()).await;
            }
            if let Some(path) = &path {
                let _ = self
                    .file_system
                    .remove(
                        path,
                        Some(&RemoveOptions {
                            recursive: None,
                            force: Some(true),
                        }),
                        context.clone(),
                    )
                    .await;
            }
            let _ = error;
        }
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending_creates
            .remove(&destination_key);
        result
    }

    /// Upstream `close` (`repo.ts:198-204`): the upstream TODO stands —
    /// repository close does not close session handles.
    pub async fn close(&self, _context: Context) -> anyhow::Result<()> {
        if self.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        Ok(())
    }

    /// Upstream `resolveCreateDestination` (`repo.ts:206-218`), minus the id
    /// minting handled inline in `create`.
    #[allow(dead_code)]
    async fn resolve_create_destination(
        &self,
        cwd_input: &str,
        context: Context,
    ) -> anyhow::Result<String> {
        file_value(
            self.file_system.absolute_path(cwd_input, context).await,
            &format!("Failed to resolve session cwd {cwd_input}"),
        )
    }

    /// Upstream `listDirectory` (`repo.ts:220-242`).
    async fn list_directory(
        &self,
        directory: &str,
        cwd: Option<&str>,
        context: Context,
    ) -> anyhow::Result<Vec<SessionMetadata>> {
        if !file_value(
            self.file_system.exists(directory, context.clone()).await,
            &format!("Failed to check sessions directory {directory}"),
        )? {
            return Ok(Vec::new());
        }
        let files: Vec<FileInfo> = file_value(
            self.file_system.list_dir(directory, context.clone()).await,
            &format!("Failed to list sessions directory {directory}"),
        )?
        .into_iter()
        .filter(|file| {
            file.kind != crate::agent_core::harness::types::FileKind::Directory
                && file.name.ends_with(".jsonl")
        })
        .collect();
        let mut metadata = Vec::new();
        for file in files {
            let Some(discovered) = self.read_session_metadata(&file, context.clone()).await? else {
                continue;
            };
            // Directory encoding is lossy: /a/b and /a-b both map to --a-b--.
            if cwd.is_none() || discovered.cwd.as_deref() == cwd {
                metadata.push(discovered);
            }
        }
        Ok(metadata)
    }

    /// Upstream `readSessionMetadata` (`repo.ts:244-257`).
    async fn read_session_metadata(
        &self,
        file: &FileInfo,
        context: Context,
    ) -> anyhow::Result<Option<SessionMetadata>> {
        let lines = file_value(
            self.file_system
                .read_text_lines(
                    &file.path,
                    Some(&crate::agent_core::harness::types::ReadTextLinesOptions {
                        max_lines: Some(1),
                    }),
                    context.clone(),
                )
                .await,
            &format!("Failed to read session header {}", file.path),
        )?;
        let Some(first_line) = lines.first() else {
            return Ok(None);
        };
        let Ok(parsed) = parse_jsonl_session_header(first_line) else {
            return Ok(None);
        };
        if parsed.is_v3() {
            let header = parsed.v3_header().expect("v3 shape").clone();
            let mut metadata =
                metadata_from_legacy_v3_header_shim(&self.file_system, &header, context).await;
            metadata.path = Some(file.path.clone());
            metadata.modified_at = Some(file.mtime_ms);
            return Ok(Some(metadata));
        }
        let header = match &parsed {
            crate::agent_core::harness::session::jsonl::codec::JsonlParsedSessionHeader::V4 {
                header,
            } => header.clone(),
            _ => unreachable!("checked is_v3 above"),
        };
        Ok(Some(metadata_from_header(
            &header,
            &file.path,
            file.mtime_ms,
        )))
    }

    /// Upstream `sessionDirectories` (`repo.ts:259-263`).
    async fn session_directories(
        &self,
        root: &str,
        context: Context,
    ) -> anyhow::Result<Vec<String>> {
        Ok(file_value(
            self.file_system.list_dir(root, context).await,
            &format!("Failed to list sessions root {root}"),
        )?
        .into_iter()
        .filter(|entry| entry.kind == crate::agent_core::harness::types::FileKind::Directory)
        .map(|entry| entry.path)
        .collect())
    }

    /// Upstream `sessionDirectory` (`repo.ts:265-270`).
    async fn session_directory(&self, cwd: &str, context: Context) -> anyhow::Result<String> {
        file_value(
            self.file_system
                .join_path(
                    &[
                        self.root(context.clone()).await?,
                        session_directory_name(cwd),
                    ],
                    context,
                )
                .await,
            &format!("Failed to resolve sessions directory for {cwd}"),
        )
    }

    /// Upstream `resolveNewSessionPath` (`repo.ts:272-283`).
    async fn resolve_new_session_path(
        &self,
        cwd: &str,
        created_at: i64,
        id: &str,
        context: Context,
    ) -> anyhow::Result<String> {
        let directory = self.session_directory(cwd, context.clone()).await?;
        self.assert_session_id_available(&directory, id, context.clone())
            .await?;
        file_value(
            self.file_system
                .create_dir(
                    &directory,
                    Some(&CreateDirOptions::default()),
                    context.clone(),
                )
                .await,
            &format!("Failed to create sessions directory {directory}"),
        )?;
        file_value(
            self.file_system
                .join_path(&[directory, session_file_name(created_at, id)], context)
                .await,
            &format!("Failed to resolve path for session {id}"),
        )
    }

    /// Upstream `assertSessionIdAvailable` (`repo.ts:285-297`).
    async fn assert_session_id_available(
        &self,
        directory: &str,
        id: &str,
        context: Context,
    ) -> anyhow::Result<()> {
        if !file_value(
            self.file_system.exists(directory, context.clone()).await,
            &format!("Failed to check sessions directory {directory}"),
        )? {
            return Ok(());
        }
        let suffix = format!("_{}.jsonl", encode_uri_component(id));
        let id_exists = file_value(
            self.file_system.list_dir(directory, context.clone()).await,
            &format!("Failed to list sessions directory {directory}"),
        )?
        .iter()
        .any(|entry| {
            entry.kind != crate::agent_core::harness::types::FileKind::Directory
                && entry.name.ends_with(&suffix)
        });
        if id_exists {
            anyhow::bail!("Session already exists: {id}");
        }
        Ok(())
    }

    /// Upstream `resolveForkInput` (`repo.ts:299-321`).
    async fn resolve_fork_input(
        &self,
        source: &SessionMetadata,
        storage: Option<&Arc<JsonlStorage>>,
        context: Context,
    ) -> anyhow::Result<JsonlForkInput> {
        if let Some(storage) = storage {
            if storage.is_legacy_v3().await {
                anyhow::bail!(
                    "Cannot fork an open legacy v3 JSONL session; commit a non-empty transaction \
                     to upgrade it to format 4 first"
                );
            }
            let next_seq = storage.capture_fork_next_seq().await?;
            return Ok(JsonlForkInput::Open {
                metadata: fork_source_metadata(source),
                next_seq,
            });
        }
        if self
            .is_legacy_v3_fork_source(source, context.clone())
            .await?
        {
            let normalized = LegacyV3Source::read(
                Arc::clone(&self.file_system),
                source.path.as_deref().unwrap_or_default(),
                context,
            )
            .await?;
            if normalized.header.id != source.id
                || normalized.header.cwd != source.cwd.as_deref().unwrap_or_default()
            {
                anyhow::bail!("Session identity does not match header: {}", source.id);
            }
            return Ok(JsonlForkInput::LegacyV3 {
                normalized: Arc::new(normalized),
            });
        }
        Ok(JsonlForkInput::Closed {
            metadata: fork_source_metadata(source),
        })
    }

    /// Upstream `isLegacyV3ForkSource` (`repo.ts:323-331`).
    async fn is_legacy_v3_fork_source(
        &self,
        source: &SessionMetadata,
        context: Context,
    ) -> anyhow::Result<bool> {
        let lines = file_value(
            self.file_system
                .read_text_lines(
                    source.path.as_deref().unwrap_or_default(),
                    Some(&crate::agent_core::harness::types::ReadTextLinesOptions {
                        max_lines: Some(1),
                    }),
                    context,
                )
                .await,
            &format!(
                "Failed to read session header {}",
                source.path.as_deref().unwrap_or_default()
            ),
        )?;
        let Some(first_line) = lines.first() else {
            return Ok(false);
        };
        let Ok(parsed) = parse_jsonl_session_header(first_line) else {
            return Ok(false);
        };
        Ok(parsed.is_v3())
    }

    /// Upstream `publishOpenSession` (`repo.ts:333-346`).
    async fn publish_open_session(
        &self,
        metadata: SessionMetadata,
        storage: Arc<JsonlStorage>,
        key: &str,
        _context: Context,
    ) -> anyhow::Result<Arc<StorageBackedSession>> {
        {
            let state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.open_sessions.contains_key(key) {
                anyhow::bail!("Session is already open: {}", metadata.id);
            }
        }
        // `onClose` removes this handle from the open registry when it is
        // still the registered storage (upstream `repo.ts:339-342`).
        let state_for_close = Arc::clone(&self.state);
        let storage_for_close = Arc::clone(&storage);
        let key_for_close = key.to_string();
        let session = Arc::new(StorageBackedSession::with_options(
            metadata.clone(),
            storage.clone() as Arc<dyn crate::agent_core::harness::session::types::Storage>,
            StorageBackedSessionOptions {
                on_close: Some(Box::new(move || {
                    let mut state = state_for_close
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if let Some(registered) = state.open_sessions.get(&key_for_close) {
                        if Arc::ptr_eq(registered, &storage_for_close) {
                            state.open_sessions.remove(&key_for_close);
                        }
                    }
                })),
                ..StorageBackedSessionOptions::default()
            },
        ));
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .open_sessions
            .insert(key.to_string(), storage);
        Ok(session)
    }

    /// Upstream `loadStorage` (`repo.ts:352-378`).
    async fn load_storage(
        &self,
        metadata: &SessionMetadata,
        context: Context,
    ) -> anyhow::Result<Arc<JsonlStorage>> {
        let path = metadata.path.as_deref().unwrap_or_default();
        if !file_value(
            self.file_system.exists(path, context.clone()).await,
            &format!("Failed to check session {path}"),
        )? {
            anyhow::bail!("Session file does not exist: {path}");
        }
        let storage = Arc::new(
            JsonlStorage::open(
                JsonlStorageOptions {
                    file_system: Arc::clone(&self.file_system),
                    path: path.to_string(),
                    now: Some(Arc::clone(&self.now)),
                },
                context.clone(),
            )
            .await?,
        );
        let result = async {
            if storage.header().id != metadata.id
                || storage.header().cwd != metadata.cwd.as_deref().unwrap_or_default()
            {
                anyhow::bail!("Session identity does not match header: {}", metadata.id);
            }
            if storage.header().storage_version != JSONL_STORAGE_VERSION {
                anyhow::bail!(
                    "Session {} uses unsupported storage version {}",
                    metadata.id,
                    storage.header().storage_version
                );
            }
            Ok(())
        }
        .await;
        if let Err(error) = result {
            let _ = storage.close(context).await;
            return Err(error);
        }
        Ok(storage)
    }
}

/// The `on_close` registry removal the upstream repo installs
/// (`repo.ts:339-342`). Implemented as a freestanding helper because the
/// closure needs the repo state; wired through the session's close gate.
macro_rules! unused_on_close_note {
    () => {};
}
unused_on_close_note!();

fn fork_source_metadata(source: &SessionMetadata) -> JsonlForkSourceMetadata {
    JsonlForkSourceMetadata {
        id: source.id.clone(),
        cwd: source.cwd.clone().unwrap_or_default(),
        path: source.path.clone().unwrap_or_default(),
    }
}

/// Forwarding shim for the legacy metadata builder (kept local to avoid a
/// circular helper import).
use crate::agent_core::harness::session::jsonl::legacy_v3::metadata_from_legacy_v3_header as metadata_from_legacy_v3_header_shim;

#[cfg(test)]
mod migration_tests;
#[cfg(test)]
pub(crate) mod tests;
