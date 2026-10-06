//! Port of `src/harness/provider.ts` (v1.0.2): the stable provider-facing
//! identity of one conversation (`pi.provider`) and the request-time
//! migration that backfills it for legacy conversations.
//!
//! Divergence (structural, disclosed): upstream types
//! `ensureProviderSessionId` against the generic `TaskRuntime<I, S, R, H>`;
//! the port takes the erased [`TaskRuntimeLike`] object the scheduler hands
//! to every task phase, and the async `(tx) => ...` commit callback becomes
//! the synchronous [`CommitChange`] closure with the created identity handed
//! back through a shared slot.

use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::agent_core::chord_support::context::Context;
use crate::ai::uuid::uuid_v7;

use super::super::documents::{define_doc, DefinitionScope, DocDefinition, DocToken};
use super::super::errors::PlainError;
use super::super::session::transaction::Transaction;
use super::super::tasks::{CommitChange, NextTaskState, TaskRuntimeLike};
use super::super::types::{DocumentFork, DocumentHistory, JsonObject};

/// The `pi.provider` state key (`provider.ts` `ProviderState.sessionId`).
const SESSION_ID_KEY: &str = "sessionId";

/// `ProviderState` (`provider.ts:8-10`): stable provider-facing identity of
/// one conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderState {
    pub session_id: String,
}

/// The built-in `pi.provider` document (`provider.ts` `ProviderDoc`); every
/// fork starts with a fresh identity instead of copying its parent
/// (`fork: "initial"`).
pub fn provider_doc() -> DocToken {
    define_doc(DocDefinition {
        kind: String::from("pi.provider"),
        version: 1,
        scope: DefinitionScope::Conversation,
        history: Some(DocumentHistory::Latest),
        fork: Some(DocumentFork::Initial),
        family: false,
        initial: Arc::new(|_| initial_json()),
        migrate: None,
        checkpoint_when: Some(Arc::new(|_, _, _| true)),
    })
    .expect("the built-in provider document definition is valid")
}

/// `initial()` (`provider.ts:18`): `{ sessionId: uuidv7() }`.
pub fn initial_json() -> JsonObject {
    let mut map = serde_json::Map::new();
    map.insert(String::from(SESSION_ID_KEY), Value::from(uuid_v7()));
    map
}

impl ProviderState {
    /// Parse from a stored document value.
    pub fn from_json(value: &JsonObject) -> Self {
        ProviderState {
            session_id: value
                .get(SESSION_ID_KEY)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }
    }
}

/// `ensureProviderSessionId` (`provider.ts:25-38`): return the persisted
/// identity without writing in the normal path. A legacy conversation without
/// `pi.provider` gets one migration commit whose `tx.doc()` runs `initial()`
/// before the provider request starts.
pub async fn ensure_provider_session_id(
    runtime: &Arc<dyn TaskRuntimeLike>,
    context: &Context,
) -> Result<String, PlainError> {
    let conversation_id = runtime.conversation_id();
    let doc = provider_doc();
    let existing = runtime
        .snapshot(
            &doc.definition,
            Some(conversation_id),
            None,
            context.clone(),
        )
        .await?;
    if let Some(existing) = existing {
        // `return existing.sessionId` — the built-in `initial()` guarantees
        // the key on every stored incarnation, so the defensive default is
        // unreachable.
        return Ok(ProviderState::from_json(&existing).session_id);
    }
    let created: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&created);
    let change: CommitChange = Box::new(move |tx: &Transaction, _current| {
        let draft = tx.doc(&doc.definition, Some(conversation_id), None, None)?;
        // `(await tx.doc(...)).sessionId`: the port's draft `read` exposes the
        // materialized value through the whole-document path (a nested-key
        // read on a freshly tracked change returns nothing), so read the
        // object and take the key.
        let value = draft
            .read(&[])
            .map_err(|error| PlainError::new(error.message()))?
            .unwrap_or_default();
        let session_id = value
            .get(SESSION_ID_KEY)
            .and_then(Value::as_str)
            .map(str::to_string);
        *slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = session_id;
        // Upstream's callback returns `undefined` (no task-state write).
        Ok(None::<NextTaskState>)
    });
    runtime.commit(change, context.clone()).await?;
    let created = created
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let Some(created) = created else {
        return Err(PlainError::new(format!(
            "Conversation {conversation_id} has no provider session ID"
        )));
    };
    Ok(created)
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;

    use serde_json::Value;
    use tokio_util::sync::CancellationToken;

    use super::super::super::documents::DocDefinition;
    use super::super::super::env::ExecutionEnv;
    use super::super::super::errors::PlainError;
    use super::super::super::harness::types::{
        ApiFuture, ContextView, ConversationHandle, ModelsHandle, RegistrySnapshotLike,
    };
    use super::super::super::ids::{ConversationId, EntryId, TaskId};
    use super::super::super::session::observation::CommittedWatch;
    use super::super::super::session::session::{create_session, Session};
    use super::super::super::storage::memory::MemoryStorage;
    use super::super::super::storage::Storage;
    use super::super::super::tasks::{HookInvoke, PlainFailure, RuntimeFuture};
    use super::super::super::types::{
        ConversationOwnership, EntryRecord, TaskOutcome, TaskRecord, TaskState,
    };
    use super::*;

    fn uuid_v7_shape() -> regex::Regex {
        regex::Regex::new("^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")
            .unwrap()
    }

    /// A task record the test commit callback receives and ignores
    /// (upstream hands the live task record).
    fn ignored_task() -> TaskRecord {
        TaskRecord {
            id: 0,
            conversation_id: 0,
            kind: String::from("probe"),
            version: 1,
            input: Value::Null,
            owner: None,
            background: false,
            abort_requested: false,
            state: TaskState::Pending {
                checkpoint: Value::Null,
            },
            memos: None,
        }
    }

    /// Minimal runtime over one Session for the migration-path tests
    /// (`snapshot` / `commit` / `conversation_id` only; every other operation
    /// is unreachable from [`ensure_provider_session_id`]).
    struct ProviderTestRuntime {
        session: Arc<Session>,
        conversation_id: ConversationId,
    }

    impl TaskRuntimeLike for ProviderTestRuntime {
        fn task_id(&self) -> TaskId {
            unreachable!("ensure_provider_session_id never reads the task id")
        }
        fn conversation_id(&self) -> ConversationId {
            self.conversation_id
        }
        fn signal(&self) -> CancellationToken {
            CancellationToken::new()
        }
        fn aborted(&self) -> bool {
            false
        }
        fn throw_if_aborted(&self) -> Result<(), PlainFailure> {
            Ok(())
        }
        fn models(&self) -> Option<Arc<dyn ModelsHandle>> {
            None
        }
        fn env(&self) -> Option<Arc<dyn ExecutionEnv>> {
            None
        }
        fn hooks(&self, _name: &str, _invoke: HookInvoke) -> RuntimeFuture<'_, ()> {
            unreachable!("ensure_provider_session_id never runs hooks")
        }
        fn registry(&self) -> Arc<dyn RegistrySnapshotLike> {
            unreachable!("ensure_provider_session_id never reads the registry")
        }
        fn commit(
            &self,
            change: CommitChange,
            context: Context,
        ) -> Pin<Box<dyn Future<Output = Result<(), PlainError>> + Send + '_>> {
            let session = Arc::clone(&self.session);
            Box::pin(async move {
                session
                    .commit(
                        move |tx| {
                            let change = change;
                            Box::pin(
                                async move { change(tx.as_ref(), &ignored_task()).map(|_| ()) },
                            )
                        },
                        context,
                    )
                    .await
            })
        }
        fn memo(
            &self,
            _name: &str,
            _candidate: Option<Value>,
            _context: Context,
        ) -> RuntimeFuture<'_, Option<Value>> {
            unreachable!("ensure_provider_session_id never reads memos")
        }
        fn sleep(
            &self,
            _until: f64,
            _context: Context,
        ) -> Pin<Box<dyn Future<Output = Result<(), PlainError>> + Send + '_>> {
            unreachable!("ensure_provider_session_id never sleeps")
        }
        fn watch_doc(
            &self,
            _definition: &DocDefinition,
            _owner: Option<ConversationId>,
            _key: Option<&str>,
            _context: Context,
        ) -> RuntimeFuture<'_, Option<Arc<CommittedWatch>>> {
            unreachable!("ensure_provider_session_id never watches documents")
        }
        fn snapshot(
            &self,
            definition: &DocDefinition,
            owner: Option<ConversationId>,
            key: Option<&str>,
            context: Context,
        ) -> RuntimeFuture<'_, Option<JsonObject>> {
            let session = Arc::clone(&self.session);
            let definition = definition.clone();
            let key = key.map(str::to_owned);
            Box::pin(async move {
                session
                    .snapshot(&definition, owner, key.as_deref(), context)
                    .await
            })
        }
        fn snapshot_as_of(
            &self,
            _definition: &DocDefinition,
            _conversation_id: ConversationId,
            _key: Option<&str>,
            _at: i64,
            _context: Context,
        ) -> Pin<Box<dyn Future<Output = Result<Option<JsonObject>, PlainError>> + Send + '_>>
        {
            unreachable!("ensure_provider_session_id never reads historical snapshots")
        }
        fn get_task(&self, _id: TaskId, _context: Context) -> ApiFuture<Option<TaskRecord>> {
            unreachable!("ensure_provider_session_id never reads tasks")
        }
        fn wait_for_task(&self, _id: TaskId, _context: Context) -> ApiFuture<TaskRecord> {
            unreachable!("ensure_provider_session_id never waits on tasks")
        }
        fn outcomes(&self, _ids: Vec<TaskId>, _context: Context) -> ApiFuture<Vec<TaskOutcome>> {
            unreachable!("ensure_provider_session_id never reads outcomes")
        }
        fn conversation(
            &self,
            _id: ConversationId,
            _context: Context,
        ) -> ApiFuture<Option<ConversationHandle>> {
            unreachable!("ensure_provider_session_id never resolves conversations")
        }
        fn entry(
            &self,
            _kind: Option<String>,
            _id: EntryId,
            _context: Context,
        ) -> ApiFuture<Option<EntryRecord>> {
            unreachable!("ensure_provider_session_id never reads entries")
        }
        fn context(
            &self,
            _id: ConversationId,
            _context: Context,
            _cutoff: Option<EntryId>,
        ) -> ApiFuture<ContextView> {
            unreachable!("ensure_provider_session_id never reads contexts")
        }
        fn now(&self) -> f64 {
            0.0
        }
        fn report(&self, _error: &PlainError) {}
        fn session(&self) -> Arc<Session> {
            Arc::clone(&self.session)
        }
    }

    async fn open_root() -> (Arc<Session>, ConversationId) {
        let storage = Arc::new(MemoryStorage::new());
        let session = create_session(Arc::clone(&storage) as Arc<dyn Storage>);
        let record = session
            .commit(
                |tx: Arc<super::super::super::session::transaction::Transaction>| {
                    Box::pin(async move { tx.create_root_conversation() })
                },
                Context::background(),
            )
            .await
            .unwrap();
        (session, record.id)
    }

    /// The built-in definition surface, pinned against the upstream
    /// `ProviderDoc` fields (`provider_identity.definition`).
    #[test]
    fn provider_doc_definition_matches_the_upstream_oracle() {
        let expected = serde_json::from_str::<Value>(
            &std::fs::read_to_string(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/durable_oracle/durable_oracle.json"
            ))
            .unwrap(),
        )
        .unwrap();
        let definition = &provider_doc().definition;
        let surface = serde_json::json!({
            "kind": definition.kind,
            "version": definition.version,
            "scope": "conversation",
            "history": "latest",
            "fork": "initial",
        });
        assert_eq!(
            surface, expected["provider_identity"]["definition"],
            "provider doc definition surface"
        );
        // `initial()` carries exactly one v7 `sessionId`.
        let initial = (definition.initial)(None);
        assert_eq!(initial.len(), 1);
        let session_id = initial.get(SESSION_ID_KEY).and_then(Value::as_str).unwrap();
        assert!(uuid_v7_shape().is_match(session_id));
        // `checkpointWhen` is always true.
        assert!((definition.checkpoint_when.as_ref().unwrap())(
            &initial,
            &[],
            super::super::super::types::CheckpointInfo {
                deltas_since_base: 0
            },
        ));
    }

    /// The lifecycle scenario (durable oracle `provider_identity.lifecycle`):
    /// a fresh conversation gets a v7 identity that is stable across reads,
    /// a legacy conversation (retired `pi.provider`) gets a fresh one from
    /// the migration commit, and a fork never inherits its parent's identity.
    #[tokio::test]
    async fn provider_identity_lifecycle_matches_the_upstream_oracle() {
        let expected = serde_json::from_str::<Value>(
            &std::fs::read_to_string(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/durable_oracle/durable_oracle.json"
            ))
            .unwrap(),
        )
        .unwrap();
        let case = &expected["provider_identity"]["lifecycle"];
        let context = Context::background();

        // Fresh conversation: the first ensure runs the migration commit; the
        // identity is stable afterwards and matches the stored document.
        let (session, root) = open_root().await;
        let runtime: Arc<dyn TaskRuntimeLike> = Arc::new(ProviderTestRuntime {
            session: Arc::clone(&session),
            conversation_id: root,
        });
        let first = ensure_provider_session_id(&runtime, &context)
            .await
            .unwrap();
        let second = ensure_provider_session_id(&runtime, &context)
            .await
            .unwrap();
        assert_eq!(first, second, "stable across requests");
        assert!(uuid_v7_shape().is_match(&first));
        let doc = provider_doc();
        let stored = session
            .snapshot(&doc.definition, Some(root), None, context.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.get(SESSION_ID_KEY).and_then(Value::as_str),
            Some(first.as_str())
        );

        // Legacy conversation: retire `pi.provider`, then the next request
        // migrates a fresh identity in.
        session
            .commit(
                |tx: Arc<super::super::super::session::transaction::Transaction>| {
                    let doc = doc.clone();
                    Box::pin(async move { tx.retire_doc(&doc.definition, Some(root), None) })
                },
                context.clone(),
            )
            .await
            .unwrap();
        let retired = session
            .snapshot(&doc.definition, Some(root), None, context.clone())
            .await
            .unwrap();
        assert!(retired.is_none(), "absent after retire");
        let migrated = ensure_provider_session_id(&runtime, &context)
            .await
            .unwrap();
        assert!(uuid_v7_shape().is_match(&migrated));
        assert_ne!(migrated, first, "fresh identity after the migration");

        // A fork starts a fresh identity instead of copying its parent.
        let entry = session
            .commit(
                |tx: Arc<super::super::super::session::transaction::Transaction>| {
                    Box::pin(async move {
                        tx.append_entry(
                            root,
                            super::super::super::types::EntryDraft::new("pi.note"),
                        )
                        .map(|record| record.id)
                    })
                },
                context.clone(),
            )
            .await
            .unwrap();
        let fork = session
            .commit(
                |tx: Arc<super::super::super::session::transaction::Transaction>| {
                    Box::pin(async move {
                        tx.fork_conversation(root, entry, ConversationOwnership::Ownerless)
                    })
                },
                context.clone(),
            )
            .await
            .unwrap();
        let fork_runtime: Arc<dyn TaskRuntimeLike> = Arc::new(ProviderTestRuntime {
            session: Arc::clone(&session),
            conversation_id: fork.id,
        });
        let fork_identity = ensure_provider_session_id(&fork_runtime, &context)
            .await
            .unwrap();
        assert!(uuid_v7_shape().is_match(&fork_identity));
        assert_ne!(fork_identity, first, "a fork gets a fresh identity");
        session.close(context).await.unwrap();

        // The oracle fixture pins the scenario's shape-level expectations
        // (raw identities are nondeterministic upstream: uuidv7).
        let mut relations: serde_json::Map<String, Value> = serde_json::Map::new();
        relations.insert(String::from("stable"), Value::from(first == second));
        relations.insert(
            String::from("migratedDiffers"),
            Value::from(migrated != first),
        );
        relations.insert(
            String::from("forkDiffers"),
            Value::from(fork_identity != first),
        );
        assert_eq!(
            Value::Object(relations),
            case["relations"],
            "lifecycle relations"
        );
    }
}
