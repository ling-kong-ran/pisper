//! Port of `src/session/session.ts`: the Session kernel — one mutation line,
//! the loaded document tracker cache, and committed publication.
//!
//! Only committed state is observable. Every commit callback, preparation,
//! Storage settlement, adoption, and publication runs while the line is held;
//! listeners run inside the line hold (upstream runs them synchronously at
//! adoption time, before the next job starts — same order, same
//! synchronization boundary under the port's mutex-based line, divergence D6).
//!
//! Divergences (structural, disclosed):
//! - **D7 (Session extension).** Upstream the Harness subclasses `SessionImpl`
//!   and overrides the `conversationCreated` / `beforeClose` protected hooks;
//!   the port composes an optional [`SessionHooks`] object instead.
//! - **D8 (async commits).** `commit_with` takes an async closure over
//!   `&Transaction`; the upstream `T | Promise<T>` union is the future's
//!   natural form.
//! - **D9 (documentState).** The `documentState()` detached state surface is
//!   resolved with [`Session::document_state`], the chord
//!   `replicatedState(source)` constructor split into
//!   [`crate::chord::api::replicated_state_from_source`] (Rust has no
//!   duck-typed overloads); `watchDoc` / `snapshot` / `snapshotAsOf` are
//!   unchanged.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use serde_json::Value;

use crate::agent_core::chord_support::context::{abort_signal_key, Context};
use crate::chord::delta::track;

use super::super::documents::{
    check_record_scope, check_record_version, document_create_of_record, materialize_document,
    resolve_address, DocDefinition,
};
use super::super::errors::PlainError;
use super::super::storage::{Storage, StorageErrorKind};
use super::super::types::{
    CommitChange, CommitPublication, ConversationRecord, DocumentAddress, DocumentCommitChange,
    DocumentPoint, DocumentRecord, DocumentScope, JsonObject, StorageWrite, TableCommitChange,
    WatchEnd, WatchEndReason,
};
use super::observation::{
    retirement_operations, CommittedStateSource, CommittedWatch, ObservedDocumentValue,
};
use super::transaction::{LoadedDocument, Transaction, TransactionHost, TransactionScope};

/// Open a Session kernel over one storage backend (`session.ts`
/// `createSession`).
pub fn create_session(storage: Arc<dyn Storage>) -> Arc<Session> {
    create_session_with_hooks(storage, None)
}

/// Open a Session kernel with the extension hooks a Harness installs
/// (upstream: `SessionImpl` subclass overrides).
pub fn create_session_with_hooks(
    storage: Arc<dyn Storage>,
    hooks: Option<Arc<dyn SessionHooks>>,
) -> Arc<Session> {
    let host = Arc::new(SessionHost {
        session: OnceLock::new(),
        storage: Arc::clone(&storage),
    });
    let session = Arc::new(Session {
        storage,
        hooks,
        host: Arc::clone(&host),
        state: Mutex::new(SessionState {
            documents: HashMap::new(),
            commit_listeners: Vec::new(),
            close_listeners: Vec::new(),
            poison: None,
        }),
        line: tokio::sync::Mutex::new(()),
        closing: AtomicBool::new(false),
        listener_ids: std::sync::atomic::AtomicU64::new(1),
    });
    // Install the host's back reference (upstream closes over `this`); a Weak
    // keeps the cycle from leaking.
    let _ = host.session.set(Arc::downgrade(&session));
    session
}

/// The protected-hook surface of `SessionImpl` (`conversationCreated` /
/// `beforeClose`). Default behavior: plain Session, stages nothing.
pub trait SessionHooks: Send + Sync {
    /// Runs inside every transaction that creates or forks a conversation,
    /// after the conversation record is staged.
    fn conversation_created(
        &self,
        _tx: &Transaction,
        _record: &ConversationRecord,
    ) -> Result<(), PlainError> {
        Ok(())
    }

    /// Runs after close seals admission and before the line closes Storage;
    /// must not fail.
    fn before_close(&self) -> Result<(), PlainError> {
        Ok(())
    }
}

/// The transaction host bridge (`this.#host` upstream): a weak back reference
/// into the owning Session.
struct SessionHost {
    session: OnceLock<Weak<Session>>,
    storage: Arc<dyn Storage>,
}

impl SessionHost {
    fn session(&self) -> Arc<Session> {
        self.session
            .get()
            .and_then(Weak::upgrade)
            .expect("session outlives its host while transactions run")
    }
}

impl TransactionHost for SessionHost {
    fn storage(&self) -> &Arc<dyn Storage> {
        &self.storage
    }

    fn cached(&self, address_id: &str) -> Option<Arc<LoadedDocument>> {
        self.session().cached_document(address_id)
    }

    fn load(
        &self,
        definition: &DocDefinition,
        address_id: &str,
        address: &DocumentAddress,
        context: &Context,
    ) -> Result<Option<Arc<LoadedDocument>>, PlainError> {
        self.session()
            .load_document(definition, address_id, address, context)
    }

    fn install(&self, document: Arc<LoadedDocument>) {
        self.session().install_document(document);
    }

    fn evict(&self, address_id: &str, record_id: i64) {
        self.session().evict_document(address_id, record_id);
    }

    fn conversation_created(
        &self,
        tx: &Transaction,
        record: &ConversationRecord,
    ) -> Result<(), PlainError> {
        match self.session().hooks.as_ref() {
            Some(hooks) => hooks.conversation_created(tx, record),
            None => Ok(()),
        }
    }
}

type CommitListener = Arc<dyn Fn(&CommitPublication, &Context) + Send + Sync>;
type CloseListener = Arc<dyn Fn() + Send + Sync>;

/// `#attachDocument`'s result: the attached observer plus its release
/// (`detach` upstream).
type AttachmentWithRelease = (Attachment, Box<dyn Fn() + Send + Sync>);

struct SessionState {
    documents: HashMap<String, Arc<LoadedDocument>>,
    commit_listeners: Vec<(u64, CommitListener)>,
    close_listeners: Vec<(u64, CloseListener)>,
    poison: Option<PlainError>,
}

/// One attached observer of a document incarnation (`#attachDocument` result).
pub enum Attachment {
    Source(Arc<CommittedStateSource>),
    Watch(Arc<CommittedWatch>),
}

impl Attachment {
    /// Forward one committed change to the observer (`observer.advance`).
    pub fn advance(
        &self,
        value: ObservedDocumentValue,
        ops: Vec<crate::chord::delta::Op>,
        context: Context,
    ) {
        match self {
            Attachment::Source(source) => source.advance(value, ops, context),
            Attachment::Watch(watch) => watch.advance(value, ops, context),
        }
    }

    /// `observer.closeSession()`.
    pub fn close_session(&self) {
        match self {
            Attachment::Source(source) => source.close_session(),
            Attachment::Watch(watch) => watch.close_session(),
        }
    }

    /// The underlying watch, for `watchDoc` callers.
    pub fn as_watch(&self) -> Option<&Arc<CommittedWatch>> {
        match self {
            Attachment::Watch(watch) => Some(watch),
            Attachment::Source(_) => None,
        }
    }
}

/// Session kernel (`session.ts` `SessionImpl`).
pub struct Session {
    storage: Arc<dyn Storage>,
    hooks: Option<Arc<dyn SessionHooks>>,
    host: Arc<SessionHost>,
    state: Mutex<SessionState>,
    /// The mutation line (`#tail` promise chain): one job at a time, FIFO
    /// (tokio's mutex is fair).
    line: tokio::sync::Mutex<()>,
    closing: AtomicBool,
    listener_ids: std::sync::atomic::AtomicU64,
}

impl Session {
    // ─── Internal cache helpers (the TransactionHost surface) ───────────

    fn cached_document(&self, address_id: &str) -> Option<Arc<LoadedDocument>> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.documents.get(address_id).cloned()
    }

    fn install_document(&self, document: Arc<LoadedDocument>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state
            .documents
            .insert(document.address_id.clone(), document);
    }

    fn evict_document(&self, address_id: &str, record_id: i64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state
            .documents
            .get(address_id)
            .is_some_and(|document| document.record.id == record_id)
        {
            state.documents.remove(address_id);
        }
    }

    /// `#loadDocument` (`session.ts:470-497`).
    fn load_document(
        &self,
        definition: &DocDefinition,
        address_id: &str,
        address: &DocumentAddress,
        context: &Context,
    ) -> Result<Option<Arc<LoadedDocument>>, PlainError> {
        {
            let state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            // A tracker serves only tokens of the version its value was
            // materialized for; others reload from Storage.
            if let Some(cached) = state.documents.get(address_id) {
                if cached.value_version == definition.version {
                    return Ok(Some(Arc::clone(cached)));
                }
            }
        }
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state
                .documents
                .get(address_id)
                .is_some_and(|cached| cached.value_version != definition.version)
            {
                state.documents.remove(address_id);
            }
        }
        let Some(record) = self
            .storage
            .find_document(address, DocumentPoint::Current, context)
            .map_err(|error| PlainError::new(error.to_string()))?
        else {
            return Ok(None);
        };
        let stored = self
            .storage
            .document(record.id, DocumentPoint::Current, context)
            .map_err(|error| PlainError::new(error.to_string()))?
            .ok_or_else(|| {
                PlainError::new(format!(
                    "Current document {} ({}) cannot be read",
                    record.id, record.kind
                ))
            })?;
        let value = materialize_document(definition, &stored)?;
        let loaded = LoadedDocument::new(
            address_id.to_string(),
            stored.record.clone(),
            stored.version,
            definition.version,
            stored.deltas_since_base,
            track(Value::Object(value)),
        );
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state
                .documents
                .insert(address_id.to_string(), Arc::clone(&loaded));
        }
        Ok(Some(loaded))
    }

    fn assert_usable(&self) -> Result<(), PlainError> {
        if self.closing.load(Ordering::SeqCst) {
            return Err(PlainError::new("Session is closed"));
        }
        self.assert_healthy()
    }

    fn assert_healthy(&self) -> Result<(), PlainError> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(poison) = &state.poison {
            return Err(PlainError::new(format!(
                "Session is poisoned by a failed commit after storage admission; reopen it (cause: {})",
                poison.message
            )));
        }
        Ok(())
    }

    // ─── Commits ────────────────────────────────────────────────────────

    // `commit(change, context)` (`session.ts:83-85`). The callback receives a
    // shared `Arc<Transaction>` so its future can be `together owned` — the
    // port-idiomatic resolution of upstream's `T | Promise<T>` union.
    pub async fn commit<T, F, Fut>(&self, change: F, context: Context) -> Result<T, PlainError>
    where
        F: FnOnce(Arc<Transaction>) -> Fut,
        Fut: std::future::Future<Output = Result<T, PlainError>> + Send + 'static,
    {
        self.commit_with(change, context, TransactionScope::default())
            .await
    }

    /// Internal commit exposing the concrete transaction and its internal
    /// operations (`commitWith`): `scope` sets the default `tx.createTask()`
    /// conversation and the task attributed to appended entries.
    pub async fn commit_with<T, F, Fut>(
        &self,
        change: F,
        context: Context,
        scope: TransactionScope,
    ) -> Result<T, PlainError>
    where
        F: FnOnce(Arc<Transaction>) -> Fut,
        Fut: std::future::Future<Output = Result<T, PlainError>> + Send + 'static,
    {
        self.assert_usable()?;
        let _line = self.line.lock().await;
        self.assert_healthy()?;
        self.run_commit(change, context, scope).await
    }

    /// `#runCommit` (`session.ts:507-547`).
    async fn run_commit<T, F, Fut>(
        &self,
        change: F,
        context: Context,
        scope: TransactionScope,
    ) -> Result<T, PlainError>
    where
        F: FnOnce(Arc<Transaction>) -> Fut,
        Fut: std::future::Future<Output = Result<T, PlainError>> + Send + 'static,
    {
        self.assert_healthy()?;
        if let Some(signal) = context.abort_signal() {
            if signal.is_cancelled() {
                return Err(PlainError::new("The operation was aborted"));
            }
        }
        let host: Arc<dyn TransactionHost> = Arc::clone(&self.host) as Arc<dyn TransactionHost>;
        let tx = Arc::new(Transaction::new(host, context.clone(), scope));
        let result = match change(Arc::clone(&tx)).await {
            Ok(result) => result,
            Err(error) => {
                tx.settle_failure();
                return Err(error);
            }
        };
        let writes = tx.settle_success()?;
        if writes.is_empty() {
            tx.discard();
            return Ok(result);
        }
        // Once admitted, caller cancellation does not interrupt Storage
        // settlement (`withoutAbortSignal(context)`).
        let cleanup = context.clone().with_value(abort_signal_key(), None);
        let seq = match self.storage.commit(&writes, &cleanup) {
            Ok(seq) => seq,
            Err(error) => {
                tx.discard();
                // Callback errors never reach this branch; StorageRejected
                // alone guarantees that no batch effect committed.
                if error.kind != StorageErrorKind::Rejected {
                    let mut state = self
                        .state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    state.poison = Some(PlainError::new(error.to_string()));
                }
                return Err(PlainError::new(error.to_string()));
            }
        };
        let documents = match tx.adopt(seq) {
            Ok(documents) => documents,
            Err(error) => {
                // Storage already committed; a failed adoption leaves memory
                // behind durable state.
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.poison = Some(PlainError::new(error.to_string()));
                return Err(error);
            }
        };
        self.publish(seq, &writes, &documents, &context);
        Ok(result)
    }

    // ─── Reads ──────────────────────────────────────────────────────────

    /// Internal: run a read-only job on the mutation line (`readOnLine`).
    pub async fn read_on_line<T, Fut, F>(&self, job: F) -> Result<T, PlainError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T, PlainError>>,
    {
        self.assert_usable()?;
        let _line = self.line.lock().await;
        self.assert_healthy()?;
        job().await
    }

    /// Internal: a conversation document's current incarnation and value, for
    /// a job already running on the line (`conversationDocumentOnLine`).
    pub fn conversation_document_on_line(
        &self,
        definition: &DocDefinition,
        conversation_id: i64,
        context: &Context,
    ) -> Result<Option<(DocumentRecord, i64, JsonObject)>, PlainError> {
        let resolved = resolve_address(definition, Some(conversation_id), None)?;
        let loaded = self.load_document(definition, &resolved.id, &resolved.address, context)?;
        let Some(loaded) = loaded else {
            return Ok(None);
        };
        check_record_scope(definition, &document_create_of_record(&loaded.record))?;
        let stored_version = loaded
            .core
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .stored_version;
        check_record_version(
            definition,
            &document_create_of_record(&loaded.record),
            stored_version,
        )?;
        Ok(Some((
            loaded.record.clone(),
            loaded.value_version,
            loaded.value().as_object().cloned().unwrap_or_default(),
        )))
    }

    /// `snapshot(token, ...)` over the resolved address (`session.ts:131-192`).
    pub async fn snapshot(
        &self,
        definition: &DocDefinition,
        owner: Option<i64>,
        key: Option<&str>,
        context: Context,
    ) -> Result<Option<JsonObject>, PlainError> {
        self.assert_usable()?;
        let resolved = resolve_address(definition, owner, key)?;
        let loaded = {
            let state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state
                .documents
                .get(&resolved.id)
                .filter(|cached| cached.value_version == definition.version)
                .cloned()
        };
        let loaded = match loaded {
            Some(loaded) => Some(loaded),
            None => {
                let _line = self.line.lock().await;
                self.assert_healthy()?;
                self.load_document(definition, &resolved.id, &resolved.address, &context)?
            }
        };
        let Some(loaded) = loaded else {
            return Ok(None);
        };
        check_record_scope(definition, &document_create_of_record(&loaded.record))?;
        let stored_version = loaded
            .core
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .stored_version;
        check_record_version(
            definition,
            &document_create_of_record(&loaded.record),
            stored_version,
        )?;
        Ok(loaded.value().as_object().cloned())
    }

    /// `snapshotAsOf` (`session.ts:331-367`).
    pub async fn snapshot_as_of(
        &self,
        definition: &DocDefinition,
        conversation_id: i64,
        key: Option<&str>,
        at: i64,
        context: Context,
    ) -> Result<Option<JsonObject>, PlainError> {
        self.assert_usable()?;
        let resolved = resolve_address(definition, Some(conversation_id), key)?;
        let scope_conversation_id = match resolved.address.scope {
            DocumentScope::Conversation { conversation_id } => conversation_id,
            _ => {
                return Err(PlainError::new(
                    "Session.snapshotAsOf() requires a conversation document",
                ));
            }
        };
        let _line = self.line.lock().await;
        self.assert_healthy()?;
        let stored_entry = self
            .storage
            .entry_visible(scope_conversation_id, at, &context)
            .map_err(|error| PlainError::new(error.to_string()))?;
        let Some(stored_entry) = stored_entry else {
            return Err(PlainError::new(format!(
                "Entry {at} is not visible from conversation {conversation_id}"
            )));
        };
        let address = DocumentAddress {
            kind: resolved.address.kind.clone(),
            scope: DocumentScope::Conversation {
                conversation_id: stored_entry.entry.conversation_id,
            },
            key: resolved.address.key.clone(),
        };
        let record = self
            .storage
            .find_document(
                &address,
                DocumentPoint::Seq(stored_entry.commit_seq),
                &context,
            )
            .map_err(|error| PlainError::new(error.to_string()))?;
        let Some(record) = record else {
            return Ok(None);
        };
        let stored = self
            .storage
            .document(
                record.id,
                DocumentPoint::Seq(stored_entry.commit_seq),
                &context,
            )
            .map_err(|error| PlainError::new(error.to_string()))?
            .ok_or_else(|| {
                PlainError::new(format!(
                    "Historical document {} ({}) cannot be read",
                    record.id, record.kind
                ))
            })?;
        Ok(Some(materialize_document(definition, &stored)?))
    }

    // ─── Observers ──────────────────────────────────────────────────────

    /// `watchDoc` over the resolved address (`session.ts:249-329`).
    pub async fn watch_doc(
        self: &Arc<Self>,
        definition: &DocDefinition,
        owner: Option<i64>,
        key: Option<&str>,
        context: Context,
    ) -> Result<Option<Arc<CommittedWatch>>, PlainError> {
        self.assert_usable()?;
        let resolved = resolve_address(definition, owner, key)?;
        let signal = context.abort_signal();
        if signal.as_ref().is_some_and(|signal| signal.is_cancelled()) {
            return Err(PlainError::new("The operation was aborted"));
        }
        let _line = self.line.lock().await;
        self.assert_healthy()?;
        let loaded = self.load_document(definition, &resolved.id, &resolved.address, &context)?;
        if signal.as_ref().is_some_and(|signal| signal.is_cancelled()) {
            return Err(PlainError::new("The operation was aborted"));
        }
        let Some(loaded) = loaded else {
            return Ok(None);
        };
        // `this.#attachDocument(...).observer` (`session.ts:285-289`): the
        // watch owns the release and unsubscribes when it terminates.
        let (attachment, _release_owned_by_watch) =
            self.attach_document(definition, &loaded, true)?;
        let watch = attachment
            .as_watch()
            .cloned()
            .expect("watch attachment carries its watch");
        if let Some(signal) = &signal {
            let watch_handle = Arc::clone(&watch);
            let cancel_token = signal.clone();
            tokio::spawn(async move {
                cancel_token.cancelled().await;
                watch_handle.cancel();
            });
        }
        Ok(Some(watch))
    }

    /// `documentState(token, ...)` over the resolved address
    /// (`session.ts:212-237`): a disposable read-only Chord state of one
    /// committed document incarnation, or `None` when the address is vacant.
    pub async fn document_state(
        self: &Arc<Self>,
        definition: &DocDefinition,
        owner: Option<i64>,
        key: Option<&str>,
        context: Context,
    ) -> Result<Option<Arc<crate::chord::services::state::AttachedReplicatedState>>, PlainError>
    {
        self.assert_usable()?;
        let resolved = resolve_address(definition, owner, key)?;
        let _line = self.line.lock().await;
        self.assert_healthy()?;
        let loaded = self.load_document(definition, &resolved.id, &resolved.address, &context)?;
        let Some(loaded) = loaded else {
            return Ok(None);
        };
        let (attachment, detach) = self.attach_document(definition, &loaded, false)?;
        let source = match &attachment {
            Attachment::Source(source) => Arc::clone(source),
            Attachment::Watch(_) => unreachable!("documentState attaches a state source"),
        };
        // `replicatedState(source)` (`session.ts:228`); a construction
        // failure releases the attachment before propagating.
        match crate::chord::api::replicated_state_from_source(
            source,
            crate::chord::types::ReplicatedStateSourceOptions::default(),
        ) {
            Ok(state) => Ok(Some(state)),
            Err(error) => {
                detach();
                Err(PlainError::new(error.message()))
            }
        }
    }

    /// `#attachDocument` (`session.ts:574-625`): attach an observer to one
    /// committed incarnation; check the definition, then forward this
    /// incarnation's committed changes and close. The returned closure is the
    /// observer's `release` (`detach` upstream): it removes both
    /// subscriptions, and the observer invokes it on its own termination.
    /// `as_watch` selects the watch constructor (`create` upstream).
    fn attach_document(
        self: &Arc<Self>,
        definition: &DocDefinition,
        loaded: &Arc<LoadedDocument>,
        as_watch: bool,
    ) -> Result<AttachmentWithRelease, PlainError> {
        check_record_scope(definition, &document_create_of_record(&loaded.record))?;
        let stored_version = loaded
            .core
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .stored_version;
        check_record_version(
            definition,
            &document_create_of_record(&loaded.record),
            stored_version,
        )?;
        let record_id = loaded.record.id;
        // The observer's release: unsubscribe once, whether invoked by the
        // observer's termination or returned to the caller. A Weak back
        // reference keeps the Session → listener → release cycle from leaking
        // (upstream relies on GC for the same cycle).
        let subscription: Arc<Mutex<Option<(u64, u64)>>> = Arc::new(Mutex::new(None));
        let make_release = |session: &Arc<Session>| -> Box<dyn Fn() + Send + Sync> {
            let session = Arc::downgrade(session);
            let subscription = Arc::clone(&subscription);
            Box::new(move || {
                if let Some((commit_id, close_id)) = subscription
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .take()
                {
                    if let Some(session) = session.upgrade() {
                        session.unsubscribe_commits(commit_id);
                        session.unsubscribe_close(close_id);
                    }
                }
            })
        };
        // A document state's frames carry no caller cancellation; a watch
        // observes its own cancellation (`frameContext` upstream — resolved
        // at the publisher since the source's frames strip the signal).
        let observed = Arc::new(AtomicI64::new(loaded.value_version));
        let attachment = if as_watch {
            Attachment::Watch(Arc::new(CommittedWatch::new(
                loaded.value(),
                make_release(self),
                None,
            )))
        } else {
            Attachment::Source(CommittedStateSource::new(
                loaded.value(),
                Box::new(make_release(self)),
            ))
        };
        let shared = match &attachment {
            Attachment::Source(source) => SharedAttachment::Source(Arc::clone(source)),
            Attachment::Watch(watch) => SharedAttachment::Watch(Arc::clone(watch)),
        };
        let subscriber: CommitListener = {
            let attachment = shared.clone();
            let observed = Arc::clone(&observed);
            Arc::new(move |publication: &CommitPublication, context: &Context| {
                for change in &publication.changes {
                    let CommitChange::Document(change) = change else {
                        continue;
                    };
                    let DocumentCommitChange::Document {
                        record,
                        version,
                        value,
                        ops,
                        ..
                    } = change
                    else {
                        continue;
                    };
                    if record.id != record_id {
                        continue;
                    }
                    let frame_ops = observed_operations(&observed, version.as_ref(), value, &ops.0);
                    // A migration-only base changes nothing for an observer
                    // of the new version (v1.0.0).
                    if frame_ops.is_empty() {
                        continue;
                    }
                    attachment.advance(value.clone(), frame_ops, context.clone());
                }
            })
        };
        let commit_id = self.subscribe_commits(subscriber)?;
        let close_id = self.subscribe_close(Arc::new({
            let attachment = shared;
            move || {
                attachment.close_session();
            }
        }))?;
        *subscription
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((commit_id, close_id));
        let detach = make_release(self);
        Ok((attachment, detach))
    }

    // ─── Close ──────────────────────────────────────────────────────────

    /// `close(context)` (`session.ts:369-385`).
    pub async fn close(&self, context: Context) -> Result<(), PlainError> {
        if !self.closing.swap(true, Ordering::SeqCst) {
            // Seal admission before anything else runs, then stop observers;
            // admitted work settles before Storage closes.
            let hooks_before_close = match self.hooks.as_ref() {
                Some(hooks) => {
                    let result = hooks.before_close();
                    result.is_err()
                }
                None => false,
            };
            let _ = hooks_before_close;
            let cleanup = context.clone().with_value(abort_signal_key(), None);
            let _line = self.line.lock().await;
            {
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.commit_listeners.clear();
                state.documents.clear();
            }
            let _ = self.storage.close(&cleanup);
            let listeners: Vec<CloseListener> = {
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state
                    .close_listeners
                    .drain(..)
                    .map(|(_, listener)| listener)
                    .collect()
            };
            for listener in listeners {
                listener();
            }
        }
        Ok(())
    }

    /// Register a synchronous post-adoption listener (`subscribeCommits`).
    /// It must not throw, block, or call Session operations. Returns the
    /// unsubscribe handle (the returned closure upstream).
    pub fn subscribe_commits(&self, listener: CommitListener) -> Result<u64, PlainError> {
        self.assert_usable()?;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let id = self.listener_ids.fetch_add(1, Ordering::SeqCst);
        state.commit_listeners.push((id, listener));
        Ok(id)
    }

    /// Remove a commit listener.
    pub fn unsubscribe_commits(&self, id: u64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state
            .commit_listeners
            .retain(|(listener_id, _)| *listener_id != id);
    }

    /// Register a listener called synchronously when close begins
    /// (`subscribeClose`).
    pub fn subscribe_close(&self, listener: CloseListener) -> Result<u64, PlainError> {
        self.assert_usable()?;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let id = self.listener_ids.fetch_add(1, Ordering::SeqCst);
        state.close_listeners.push((id, listener));
        Ok(id)
    }

    /// Remove a close listener.
    pub fn unsubscribe_close(&self, id: u64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state
            .close_listeners
            .retain(|(listener_id, _)| *listener_id != id);
    }

    /// Drop every loaded tracker on the mutation line; later access cold-loads
    /// from Storage (`unloadDocuments`).
    pub async fn unload_documents(&self) {
        let _line = self.line.lock().await;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.documents.clear();
    }

    /// `#publish` (`session.ts:549-572`).
    fn publish(
        &self,
        seq: i64,
        writes: &[StorageWrite],
        documents: &[DocumentCommitChange],
        context: &Context,
    ) {
        let listeners: Vec<CommitListener> = {
            let state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.commit_listeners.is_empty() {
                return;
            }
            state
                .commit_listeners
                .iter()
                .map(|(_, listener)| Arc::clone(listener))
                .collect()
        };
        let mut changes: Vec<CommitChange> = Vec::new();
        for write in writes {
            match write {
                StorageWrite::Conversation { value } => {
                    changes.push(CommitChange::Table(TableCommitChange::Conversation {
                        value: value.clone(),
                    }))
                }
                StorageWrite::Entry { value } => {
                    changes.push(CommitChange::Table(TableCommitChange::Entry {
                        value: value.clone(),
                    }))
                }
                StorageWrite::Task { value } => {
                    changes.push(CommitChange::Table(TableCommitChange::Task {
                        value: value.clone(),
                    }))
                }
                StorageWrite::Submission { value } => {
                    changes.push(CommitChange::Table(TableCommitChange::Submission {
                        value: value.clone(),
                    }))
                }
                _ => {}
            }
        }
        for document in documents {
            changes.push(CommitChange::Document(document.clone()));
        }
        let publication = CommitPublication { seq, changes };
        for listener in listeners {
            listener(&publication, context);
        }
    }
}

/// Type-erased clonable attachment handle shared into the commit and close
/// listeners.
#[derive(Clone)]
enum SharedAttachment {
    Source(Arc<CommittedStateSource>),
    Watch(Arc<CommittedWatch>),
}

impl SharedAttachment {
    fn advance(
        &self,
        value: ObservedDocumentValue,
        ops: Vec<crate::chord::delta::Op>,
        context: Context,
    ) {
        match self {
            SharedAttachment::Source(source) => source.advance(value, ops, context),
            SharedAttachment::Watch(watch) => watch.advance(value, ops, context),
        }
    }

    fn close_session(&self) {
        match self {
            SharedAttachment::Source(source) => source.close_session(),
            SharedAttachment::Watch(watch) => watch.close_session(),
        }
    }
}

/// `observedOperations` (`session.ts:583-598`): operations an observer applies
/// for one committed change. An observer hydrated under another definition
/// version holds a differently shaped value, so it receives the new value as a
/// root replacement instead of operations for that shape.
fn observed_operations(
    observed: &AtomicI64,
    version: Option<&i64>,
    value: &ObservedDocumentValue,
    ops: &[crate::chord::delta::Op],
) -> Vec<crate::chord::delta::Op> {
    if value.is_null() {
        return retirement_operations();
    }
    let Some(version) = version else {
        return vec![crate::chord::delta::Op::Replace(value.clone())];
    };
    let observed_version = observed.load(Ordering::SeqCst);
    if *version == observed_version {
        return ops.to_vec();
    }
    observed.store(*version, Ordering::SeqCst);
    vec![crate::chord::delta::Op::Replace(value.clone())]
}

impl WatchEnd {
    /// The terminal reason, when the watch ended for a reason.
    pub fn reason(&self) -> Option<WatchEndReason> {
        match self {
            WatchEnd::Reason(reason) => Some(*reason),
            WatchEnd::ListenerError { .. } => None,
        }
    }
}
