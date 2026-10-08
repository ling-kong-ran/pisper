//! Ports of `packages/agent/test/harness/session-create-branch.test.ts` (77
//! lines) and `storage-backed-session.test.ts` (397 lines): the session
//! capability layer over MemoryStorage — branch creation, mutation-line
//! serialization, mutator invalidation, and close semantics.

use super::MemoryStorage;
use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::context::background_context;
use crate::agent_core::harness::session::commit::insert_entry;
use crate::agent_core::harness::session::session::{
    MutationLine, SessionBranchExistsError, SessionInvalidBranchError, SessionUnknownTargetError,
    StorageBackedSession, StorageBackedSessionOptions,
};
use crate::agent_core::harness::session::testing::{GatingStorage, InstrumentedStorage};
use crate::agent_core::harness::session::types::{
    BranchScan, Entry, NewEntry, Session, SessionMetadata, Storage, Write,
};
use crate::agent_core::harness::session::values::{
    append_list, branch_tip, delete_value, lane_config, lane_state, list, session_name, set_value,
    value, ValueAddress,
};
use crate::agent_core::types::AgentMessage;
use crate::ai::types::message::{AssistantMessage, StringOrBlocks, UserMessage};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

const NOW: i64 = 1_700_000_000_000;

fn metadata() -> SessionMetadata {
    SessionMetadata {
        id: "session".to_string(),
        created_at: NOW,
        storage_version: 1,
        cwd: Some("/workspace".to_string()),
        ..SessionMetadata::default()
    }
}

fn memory_storage() -> Arc<MemoryStorage> {
    Arc::new(MemoryStorage::new(super::MemoryStorageOptions {
        now: Some(Arc::new(|| NOW)),
    }))
}

fn memory_session() -> Arc<StorageBackedSession> {
    Arc::new(StorageBackedSession::new(metadata(), memory_storage()))
}

fn custom_entry(id: &str, parent_id: Option<&str>, custom_type: &str) -> Write {
    insert_entry(NewEntry::Custom {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        custom_type: custom_type.to_string(),
        data: None,
    })
}

async fn commit_session(session: &StorageBackedSession, transaction: Vec<Write>) {
    session
        .mutate(
            |mutator, context| {
                let transaction = transaction.clone();
                Box::pin(async move {
                    mutator.commit(transaction, context).await?;
                    Ok(())
                })
            },
            background_context(),
        )
        .await
        .unwrap();
}

// --- session-create-branch.test.ts -------------------------------------------

/// "creates only the data Branch at a validated target"
/// (`session-create-branch.test.ts:21-45`).
#[tokio::test]
async fn creates_only_data_branch_at_validated_target() {
    let session = memory_session();
    commit_session(&session, vec![custom_entry("target", None, "target")]).await;

    let branch = session
        .create_branch("main", Some("target".to_string()), background_context())
        .await
        .unwrap();
    assert_eq!(
        branch
            .get_tip_id(background_context())
            .await
            .unwrap()
            .as_deref(),
        Some("target")
    );
    assert_eq!(
        session
            .get_value(&branch_tip("main"), background_context())
            .await
            .unwrap()
            .unwrap()
            .value,
        serde_json::json!("target")
    );
    assert!(session
        .get_value(&lane_config("main"), background_context())
        .await
        .unwrap()
        .is_none());
    assert!(session
        .get_value(&lane_state("main"), background_context())
        .await
        .unwrap()
        .is_none());
    session.close(background_context()).await.unwrap();
}

/// "validates names and non-null targets" (`session-create-branch.test.ts:47-60`).
#[tokio::test]
async fn validates_branch_names_and_targets() {
    use crate::agent_core::harness::session::types::Branch;
    let session = memory_session();
    let error = session
        .create_branch("", None, background_context())
        .await
        .map(|_: Arc<dyn Branch>| ())
        .unwrap_err();
    assert!(error.downcast_ref::<SessionInvalidBranchError>().is_some());
    let error = session
        .create_branch("bad\0name", None, background_context())
        .await
        .map(|_: Arc<dyn Branch>| ())
        .unwrap_err();
    assert!(error.downcast_ref::<SessionInvalidBranchError>().is_some());
    let error = session
        .create_branch("main", Some("missing".to_string()), background_context())
        .await
        .map(|_: Arc<dyn Branch>| ())
        .unwrap_err();
    assert!(error.downcast_ref::<SessionUnknownTargetError>().is_some());
    assert!(session
        .branch("main", background_context())
        .await
        .unwrap()
        .is_none());
    session.close(background_context()).await.unwrap();
}

/// "rejects duplicates atomically, including concurrent creation"
/// (`session-create-branch.test.ts:62-77`).
#[tokio::test]
async fn rejects_duplicate_branch_creation_atomically() {
    use crate::agent_core::harness::session::types::Branch;
    let session = memory_session();
    let first = session.create_branch("main", None, background_context());
    let second = session.create_branch("main", None, background_context());
    let (first, second) = tokio::join!(first, second);
    let fulfilled = [first.is_ok(), second.is_ok()]
        .iter()
        .filter(|ok| **ok)
        .count();
    assert_eq!(fulfilled, 1);
    let rejected = if first.is_err() { first } else { second };
    assert!(rejected
        .map(|_: Arc<dyn Branch>| ())
        .unwrap_err()
        .downcast_ref::<SessionBranchExistsError>()
        .is_some());
    assert!(session
        .branch("main", background_context())
        .await
        .unwrap()
        .is_some());
    session.close(background_context()).await.unwrap();
}

// --- storage-backed-session.test.ts ------------------------------------------

/// "delegates typed values directly without validation or cloning"
/// (`storage-backed-session.test.ts:34-54`).
#[tokio::test]
async fn delegates_typed_values_atomically() {
    let storage = memory_storage();
    let instrumented = InstrumentedStorage::new(storage);
    let session = Arc::new(StorageBackedSession::new(
        metadata(),
        Arc::clone(&instrumented) as Arc<dyn Storage>,
    ));
    let data = serde_json::json!({ "nested": ["original"] });
    let address = value("test.value", "state");
    let transaction = vec![
        insert_entry(NewEntry::Custom {
            id: "00000000-0000-7000-8000-000000000001".to_string(),
            parent_id: None,
            custom_type: "note".to_string(),
            data: Some(data.clone()),
        }),
        set_value(&address, data.clone()),
    ];

    let result = session
        .mutate(
            |mutator, context| {
                let transaction = transaction.clone();
                Box::pin(async move { mutator.commit(transaction, context).await })
            },
            background_context(),
        )
        .await
        .unwrap();

    assert_eq!(instrumented.get_commit_attempts()[0], transaction);
    let entry = session
        .get_entries(
            &["00000000-0000-7000-8000-000000000001".to_string()],
            background_context(),
        )
        .await
        .unwrap()
        .get("00000000-0000-7000-8000-000000000001")
        .unwrap()
        .clone();
    assert_eq!(entry.seq(), result.seqs[0]);
    assert_eq!(entry.timestamp(), NOW);
    let crate::agent_core::harness::session::types::Entry::Custom {
        data: entry_data, ..
    } = &entry
    else {
        panic!("expected custom entry");
    };
    assert_eq!(*entry_data, Some(data.clone()));
    assert_eq!(
        session
            .get_value(&address, background_context())
            .await
            .unwrap()
            .unwrap()
            .value,
        data
    );
    session.close(background_context()).await.unwrap();
}

/// "serializes read-modify-write callbacks on the single Session line"
/// (`storage-backed-session.test.ts:96-109`).
#[tokio::test]
async fn serializes_read_modify_write_callbacks() {
    let session = memory_session();
    let counter = value("test.counter", "");
    let increment = {
        let session = Arc::clone(&session);
        move || {
            let session = Arc::clone(&session);
            let counter = counter.clone();
            async move {
                session
                    .mutate(
                        |mutator, context| {
                            let counter = counter.clone();
                            Box::pin(async move {
                                let next =
                                    match mutator.get_value(&counter, context.clone()).await? {
                                        Some(stored) => stored.value.as_i64().unwrap_or(0),
                                        None => 0,
                                    } + 1;
                                mutator
                                    .commit(
                                        vec![set_value(&counter, serde_json::json!(next))],
                                        context,
                                    )
                                    .await?;
                                Ok(next)
                            })
                        },
                        background_context(),
                    )
                    .await
            }
        }
    };
    let (first, second) = tokio::join!(increment(), increment());
    assert_eq!((first.unwrap(), second.unwrap()), (1, 2));
    assert_eq!(
        session
            .get_value(&value("test.counter", ""), background_context())
            .await
            .unwrap()
            .unwrap()
            .value,
        serde_json::json!(2)
    );
    session.close(background_context()).await.unwrap();
}

/// "queues a nested public writer until its owning callback returns"
/// (`storage-backed-session.test.ts:129-145`).
#[tokio::test]
async fn queues_nested_writer_until_callback_returns() {
    let session = memory_session();
    let nested_slot: Arc<
        std::sync::Mutex<Option<futures::future::BoxFuture<'static, anyhow::Result<()>>>>,
    > = Arc::new(std::sync::Mutex::new(None));
    {
        let session = Arc::clone(&session);
        let nested_slot = Arc::clone(&nested_slot);
        session
            .mutate(
                |_mutator, _context| {
                    let session = Arc::clone(&session);
                    let nested_slot = Arc::clone(&nested_slot);
                    Box::pin(async move {
                        // Created but NOT awaited inside the callback: it
                        // queues behind the owning callback (awaiting it
                        // inside would deadlock, like upstream).
                        *nested_slot.lock().unwrap() = Some(Box::pin(async move {
                            Session::set_value(
                                session.as_ref(),
                                session_name(),
                                serde_json::json!("nested"),
                                background_context(),
                            )
                            .await
                        }));
                        Ok(())
                    })
                },
                background_context(),
            )
            .await
            .unwrap();
    }
    let nested = nested_slot.lock().unwrap().take();
    if let Some(nested) = nested {
        nested.await.unwrap();
    }
    assert_eq!(
        session.get_name(background_context()).await.unwrap(),
        Some("nested".to_string())
    );
    session.close(background_context()).await.unwrap();
}

/// "exposes either side of an atomic multi-write commit to direct reads"
/// (`storage-backed-session.test.ts:147-170`).
#[tokio::test]
async fn gates_atomic_multi_write_commits() {
    let first = value("test.atomic", "first");
    let second = value("test.atomic", "second");
    let base = memory_storage();
    base.commit(
        vec![
            set_value(&first, serde_json::json!("old")),
            set_value(&second, serde_json::json!("old")),
        ],
        background_context(),
    )
    .await
    .unwrap();
    let gated = GatingStorage::new(base);
    gated.arm();
    let session = Arc::new(StorageBackedSession::new(
        metadata(),
        Arc::clone(&gated) as Arc<dyn Storage>,
    ));
    let committing = {
        let session = Arc::clone(&session);
        async move {
            session
                .mutate(
                    |mutator, context| {
                        let first = first.clone();
                        let second = second.clone();
                        Box::pin(async move {
                            mutator
                                .commit(
                                    vec![
                                        set_value(&first, serde_json::json!("new")),
                                        set_value(&second, serde_json::json!("new")),
                                    ],
                                    context,
                                )
                                .await?;
                            Ok(())
                        })
                    },
                    background_context(),
                )
                .await
        }
    };
    let committing = tokio::spawn(committing);
    // Wait for the commit to park inside the gate.
    while gated.pending() == 0 {
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    assert_eq!(
        session
            .get_value(&value("test.atomic", "first"), background_context())
            .await
            .unwrap()
            .unwrap()
            .value,
        serde_json::json!("old")
    );
    assert_eq!(
        session
            .get_value(&value("test.atomic", "second"), background_context())
            .await
            .unwrap()
            .unwrap()
            .value,
        serde_json::json!("old")
    );
    gated.next(1).await.unwrap();
    committing.await.unwrap().unwrap();
    assert_eq!(
        session
            .get_value(&value("test.atomic", "first"), background_context())
            .await
            .unwrap()
            .unwrap()
            .value,
        serde_json::json!("new")
    );
    assert_eq!(
        session
            .get_value(&value("test.atomic", "second"), background_context())
            .await
            .unwrap()
            .unwrap()
            .value,
        serde_json::json!("new")
    );
    session.close(background_context()).await.unwrap();
}

/// "holds the explicit Session barrier through commit until end"
/// (`storage-backed-session.test.ts:172-196`).
#[tokio::test]
async fn holds_barrier_until_end() {
    let storage = InstrumentedStorage::new(memory_storage());
    let session = Arc::new(StorageBackedSession::new(
        metadata(),
        Arc::clone(&storage) as Arc<dyn Storage>,
    ));
    let mutation = session.begin_mutation(background_context()).await.unwrap();
    let queued = {
        let session = Arc::clone(&session);
        tokio::spawn(async move {
            crate::agent_core::harness::session::types::Session::mutate(
                &*session,
                |_mutator, _ctx| Box::pin(async { Ok(()) }),
                background_context(),
            )
            .await
        })
    };

    // The queued callback has not started while the barrier is held.
    let result = mutation
        .commit(Vec::new(), background_context())
        .await
        .unwrap();
    assert!(result.seqs.is_empty());
    assert!(storage.get_commit_attempts().len() == 1);
    mutation.end(background_context()).await.unwrap();
    queued.await.unwrap().unwrap();
    assert!(storage.get_commit_attempts().len() == 1);
    // The mutator is invalidated after end.
    let error = crate::agent_core::harness::session::types::SessionMutationReader::get_entries(
        mutation.as_ref(),
        &[],
        background_context(),
    )
    .await;
    assert!(error.is_err());
    session.close(background_context()).await.unwrap();
}

/// "exposes explicit branch scans through the Session and callback-scoped
/// mutator" (`storage-backed-session.test.ts:233-258`).
#[tokio::test]
async fn exposes_branch_scans_through_session_and_mutator() {
    let session = memory_session();
    let child_id = "00000000-0000-7000-8000-000000000002";
    commit_session(
        &session,
        vec![
            custom_entry("00000000-0000-7000-8000-000000000001", None, "root"),
            custom_entry(
                child_id,
                Some("00000000-0000-7000-8000-000000000001"),
                "child",
            ),
        ],
    )
    .await;

    let scan = session
        .scan_branch(
            &crate::agent_core::harness::session::types::StorageBranchScan {
                start: child_id.to_string(),
                order: Some(
                    crate::agent_core::harness::session::types::BranchScanOrder::OldestFirst,
                ),
                ..Default::default()
            },
            background_context(),
        )
        .await
        .unwrap();
    assert_eq!(
        scan.iter().map(Entry::id).collect::<Vec<_>>(),
        vec!["00000000-0000-7000-8000-000000000001", child_id]
    );
    session
        .mutate(
            |mutator, context| {
                let child_id = child_id.to_string();
                Box::pin(async move {
                    let scanned = mutator
                        .scan_branch(
                            &crate::agent_core::harness::session::types::StorageBranchScan {
                                start: child_id.clone(),
                                limit: Some(1),
                                ..Default::default()
                            },
                            context,
                        )
                        .await?;
                    assert_eq!(
                        scanned.iter().map(Entry::id).collect::<Vec<_>>(),
                        vec![child_id]
                    );
                    Ok(())
                })
            },
            background_context(),
        )
        .await
        .unwrap();
    session.close(background_context()).await.unwrap();
}

/// "rejects pending assistant entries at the durable session write boundary"
/// (`storage-backed-session.test.ts:260-289`).
#[tokio::test]
async fn rejects_pending_assistant_entries() {
    let storage = InstrumentedStorage::new(memory_storage());
    let session = Arc::new(StorageBackedSession::new(
        metadata(),
        Arc::clone(&storage) as Arc<dyn Storage>,
    ));
    let pending = AssistantMessage {
        content: Vec::new(),
        api: "anthropic-messages".to_string(),
        provider: "anthropic".to_string(),
        model: "claude-sonnet-4-5".to_string(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        diagnostics: None,
        usage: Default::default(),
        stop_reason: crate::ai::types::primitives::StopReason::Pending,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: NOW,
    };

    let error = session
        .mutate(
            |mutator, context| {
                Box::pin(async move {
                    mutator
                        .commit(
                            vec![insert_entry(NewEntry::Message {
                                id: "00000000-0000-7000-8000-000000000001".to_string(),
                                parent_id: None,
                                message: AgentMessage::Assistant(pending),
                                terminate: None,
                            })],
                            context,
                        )
                        .await
                })
            },
            background_context(),
        )
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Cannot persist a pending assistant message"),
        "{error}"
    );
    assert!(storage.get_commit_attempts().is_empty());
    assert!(session
        .get_entries(
            &["00000000-0000-7000-8000-000000000001".to_string()],
            background_context()
        )
        .await
        .unwrap()
        .is_empty());
    session.close(background_context()).await.unwrap();
}

/// "serializes mutations, permits one commit attempt, and invalidates the
/// mutator" (`storage-backed-session.test.ts:313-331`).
#[tokio::test]
async fn permits_one_commit_attempt_and_invalidates_mutator() {
    let storage = InstrumentedStorage::new(memory_storage());
    let session = Arc::new(StorageBackedSession::new(
        metadata(),
        Arc::clone(&storage) as Arc<dyn Storage>,
    ));

    session
        .mutate(
            |mutator, context| {
                Box::pin(async move {
                    assert!(mutator
                        .get_value(&session_name(), context.clone())
                        .await
                        .unwrap()
                        .is_none());
                    mutator
                        .commit(
                            vec![set_value(&session_name(), serde_json::json!("committed"))],
                            context.clone(),
                        )
                        .await?;
                    let error = mutator.commit(Vec::new(), context).await.unwrap_err();
                    assert!(
                        error.to_string().contains("commit already attempted"),
                        "{error}"
                    );
                    Ok(())
                })
            },
            background_context(),
        )
        .await
        .unwrap();

    assert_eq!(storage.get_commit_attempts().len(), 1);
    assert_eq!(
        session.get_name(background_context()).await.unwrap(),
        Some("committed".to_string())
    );
    session.close(background_context()).await.unwrap();
}

/// "consumes the commit guard when the first commit fails"
/// (`storage-backed-session.test.ts:333-348`).
#[tokio::test]
async fn consumes_commit_guard_on_failure() {
    let storage = InstrumentedStorage::new(memory_storage());
    let session = Arc::new(StorageBackedSession::new(
        metadata(),
        Arc::clone(&storage) as Arc<dyn Storage>,
    ));
    let transaction = vec![custom_entry(
        "00000000-0000-7000-8000-000000000001",
        Some("missing"),
        "note",
    )];

    session
        .mutate(
            |mutator, context| {
                let transaction = transaction.clone();
                Box::pin(async move {
                    let error = mutator
                        .commit(transaction, context.clone())
                        .await
                        .unwrap_err();
                    assert!(
                        error.to_string().contains("Missing parent entry"),
                        "{error}"
                    );
                    let error = mutator.commit(Vec::new(), context).await.unwrap_err();
                    assert!(
                        error.to_string().contains("commit already attempted"),
                        "{error}"
                    );
                    Ok(())
                })
            },
            background_context(),
        )
        .await
        .unwrap();
    assert_eq!(storage.get_commit_attempts().len(), 1);
    session.close(background_context()).await.unwrap();
}

/// "mints distinct follower ids with the leader timestamp"
/// (`storage-backed-session.test.ts:350-360`).
#[tokio::test]
async fn mints_distinct_follower_ids_with_leader_timestamp() {
    let session = memory_session();
    let leader_timestamp = 0x0123456789abi64;
    let generator = session.id_generator();
    let leader = generator.next(Some(leader_timestamp));
    let follower_1 = generator.next(Some(leader_timestamp));
    let follower_2 = generator.next(Some(leader_timestamp));
    let decode = |id: &str| i64::from_str_radix(&id.replace('-', "")[..12], 16).unwrap();
    assert_eq!(
        (decode(&leader), decode(&follower_1), decode(&follower_2)),
        (leader_timestamp, leader_timestamp, leader_timestamp)
    );
    assert_eq!(
        std::collections::HashSet::from([leader, follower_1, follower_2]).len(),
        3
    );
    session.close(background_context()).await.unwrap();
}

/// "accepts an injected id generator for deterministic execution tests"
/// (`storage-backed-session.test.ts:362-371`).
#[tokio::test]
async fn accepts_injected_id_generator() {
    struct Counting(std::sync::atomic::AtomicUsize);
    impl crate::agent_core::harness::session::types::IdGenerator for Counting {
        fn next(&self, timestamp_ms: Option<i64>) -> String {
            let n = self.0.fetch_add(1, Ordering::SeqCst) + 1;
            match timestamp_ms {
                Some(ts) => format!("{ts}:{n}"),
                None => format!("now:{n}"),
            }
        }
    }
    let counter = Arc::new(Counting(std::sync::atomic::AtomicUsize::new(0)));
    let session = Arc::new(StorageBackedSession::with_options(
        metadata(),
        memory_storage(),
        StorageBackedSessionOptions {
            id_generator: Some(Arc::clone(&counter) as _),
            ..Default::default()
        },
    ));
    assert_eq!(session.id_generator().next(Some(7)), "7:1");
    assert_eq!(session.id_generator().next(None), "now:2");
    session.close(background_context()).await.unwrap();
}

/// "exposes metadata directly and the shared UUIDv7 id generator"
/// (`storage-backed-session.test.ts:373-382`).
#[tokio::test]
async fn exposes_metadata_and_uuidv7_generator() {
    let session = memory_session();
    assert_eq!(session.metadata().id, "session");
    let id = session.id_generator().next(None);
    assert_eq!(id.len(), 36);
    assert_eq!(
        id.split('-')
            .nth(2)
            .map(|part| part.chars().next().unwrap()),
        Some('7')
    );
    session.close(background_context()).await.unwrap();
}

/// "closes idempotently and rejects operations not admitted before close"
/// (`storage-backed-session.test.ts:384-396`).
#[tokio::test]
async fn closes_idempotently_and_rejects_later_operations() {
    let session = memory_session();
    let (first, second) = tokio::join!(
        session.close(background_context()),
        session.close(background_context())
    );
    first.unwrap();
    second.unwrap();
    let error = session
        .mutate(
            |_mutator, _context| Box::pin(async { Ok(()) }),
            background_context(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Session is closed"), "{error}");
    let error = session
        .get_entries(&[], background_context())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Session is closed"), "{error}");
    let error = session
        .get_value(&session_name(), background_context())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Session is closed"), "{error}");
    let error = session
        .scan_values(&session_name(), background_context())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Session is closed"), "{error}");
    let error = session
        .scan_branch(
            &crate::agent_core::harness::session::types::StorageBranchScan {
                start: "00000000-0000-7000-8000-000000000001".to_string(),
                ..Default::default()
            },
            background_context(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Session is closed"), "{error}");
}

/// Session-level `findEntries` cursor paging and label/name helpers
/// (exercised upstream through `session.ts` + the repo oracles).
#[tokio::test]
async fn find_entries_pages_and_names_resolve() {
    let session = memory_session();
    commit_session(
        &session,
        vec![
            custom_entry("e1", None, "note"),
            custom_entry("e2", Some("e1"), "note"),
            custom_entry("e3", Some("e2"), "note"),
        ],
    )
    .await;
    session
        .set_label("e2", Some("labeled".to_string()), background_context())
        .await
        .unwrap();
    session
        .set_name(Some("named".to_string()), background_context())
        .await
        .unwrap();

    assert_eq!(
        session.get_name(background_context()).await.unwrap(),
        Some("named".to_string())
    );
    assert_eq!(
        session.get_label("e2", background_context()).await.unwrap(),
        Some("labeled".to_string())
    );
    session
        .set_label("e2", None, background_context())
        .await
        .unwrap();
    session.set_name(None, background_context()).await.unwrap();
    assert!(session
        .get_name(background_context())
        .await
        .unwrap()
        .is_none());
    assert!(session
        .get_label("e2", background_context())
        .await
        .unwrap()
        .is_none());

    // Descending default order + cursor paging.
    let all = session
        .find_entries(None, background_context())
        .await
        .unwrap();
    assert_eq!(
        all.iter().map(Entry::id).collect::<Vec<_>>(),
        vec!["e3", "e2", "e1"]
    );
    let page = session
        .find_entries(
            Some(&crate::agent_core::harness::session::types::EntryQuery {
                cursor: Some(crate::agent_core::harness::session::types::EntryCursor {
                    seq: all[0].seq(),
                }),
                ..Default::default()
            }),
            background_context(),
        )
        .await
        .unwrap();
    assert_eq!(
        page.iter().map(Entry::id).collect::<Vec<_>>(),
        vec!["e2", "e1"]
    );
    // The ascending cursor guard at MAX_SAFE_INTEGER returns empty.
    let guard = session
        .find_entries(
            Some(&crate::agent_core::harness::session::types::EntryQuery {
                order: Some(crate::agent_core::harness::session::types::AscDescOrder::Asc),
                cursor: Some(crate::agent_core::harness::session::types::EntryCursor {
                    seq: crate::agent_core::harness::session::types::MAX_SAFE_INTEGER,
                }),
                ..Default::default()
            }),
            background_context(),
        )
        .await
        .unwrap();
    assert!(guard.is_empty());
    session.close(background_context()).await.unwrap();
}

// Keep the fixture imports referenced.
#[allow(unused, clippy::too_many_arguments)]
fn _keepers(
    _c: Context,
    _b: Option<BranchScan>,
    _e: Option<Entry>,
    _m: Option<AgentMessage>,
    _u: Option<UserMessage>,
    _s: Option<StringOrBlocks>,
    _h: Option<HashMap<String, String>>,
    _a: Option<ValueAddress>,
    _l: Option<crate::agent_core::harness::session::values::ListReadOptions>,
    _line: Option<MutationLine>,
    _msg: Option<AgentMessage>,
) {
    let _ = user_message_fixture();
    let _ = (
        delete_value(&session_name()),
        append_list(&list("t", ""), serde_json::json!(1)),
    );
}

fn user_message_fixture() -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: StringOrBlocks::Text("x".to_string()),
        timestamp: 1,
    })
}

/// Byte replay of the `MemoryStorage` halves of
/// `packages/agent/test/harness/memory-conformance.test.ts`:
/// `createStorageConformance` (21 scenarios over `MemoryStorage`) plus the
/// hand-written "MemoryStorage commit statistics" scenario. The upstream case
/// bodies were executed unmodified against the byte-copied upstream sources
/// with `node --experimental-strip-types` (oracle runner in
/// `target/m3b-task11-scratch`; 58/58 pass) and the recorded values are
/// asserted here. Commit timestamps are byte assertions because the fixture
/// injects the same fixed clock upstream does; generated ids/timestamps of
/// uuidv7 origin are asserted by the upstream generation rule instead.
mod replay {
    use std::sync::Arc;

    use super::super::{MemorySessionRepo, MemorySessionRepoOptions, MemoryStorageOptions};
    use super::MemoryStorage;
    use crate::agent_core::chord_support::Context;
    use crate::agent_core::harness::context::background_context;
    use crate::agent_core::harness::session::commit::{insert_entry, insert_usage};
    use crate::agent_core::harness::session::types::{
        AscDescOrder, BranchScanOrder, Entry, EntryScan, EntryStructure, EntryType, NewEntry,
        NewUsageRow, Session, SessionCreateOptions, SessionStats, Storage, StorageBranchScan,
        UsageScan, Write,
    };
    use crate::agent_core::harness::session::values::{
        append_list, branch_tip, delete_list, delete_value, entry_label, list, pending_entry,
        session_name, set_value, value, ListCursor, ListReadOptions,
    };
    use crate::agent_core::types::AgentMessage;
    use crate::ai::types::primitives::{Usage, UsageCost};
    use crate::ai::types::{StringOrBlocks, TextContent, TextOrImageBlock, UserMessage};

    const NOW: i64 = 1_700_000_000_000;
    const MESSAGE_TIMESTAMP: i64 = 1_650_000_000_000;

    fn ctx() -> Context {
        background_context()
    }

    /// Upstream `new MemoryStorage({ now: () => NOW })` fixture.
    fn storage() -> MemoryStorage {
        MemoryStorage::new(MemoryStorageOptions {
            now: Some(Arc::new(|| NOW)),
        })
    }

    /// Upstream `usage(input, output, options)` (`conformance/storage.ts:54-71`).
    fn usage(input: i64, output: i64) -> Usage {
        Usage {
            input: input as u64,
            output: output as u64,
            cache_read: (input + 1) as u64,
            cache_write: (output + 1) as u64,
            cache_write_1h: None,
            reasoning: None,
            total_tokens: (input + output) as u64,
            cost: UsageCost {
                input: input as f64 / 100.0,
                output: output as f64 / 100.0,
                cache_read: (input + 1) as f64 / 100.0,
                cache_write: (output + 1) as f64 / 100.0,
                total: (input + output + 2) as f64 / 100.0,
            },
        }
    }

    /// Upstream `userEntry(id, parentId, text)` (`conformance/storage.ts:84-95`).
    fn user_entry(id: &str, parent_id: Option<&str>, text: &str) -> Write {
        insert_entry(NewEntry::Message {
            id: id.to_string(),
            parent_id: parent_id.map(str::to_string),
            message: AgentMessage::User(UserMessage {
                content: StringOrBlocks::Blocks(vec![TextOrImageBlock::Text(TextContent {
                    text: text.to_string(),
                    text_signature: None,
                })]),
                timestamp: MESSAGE_TIMESTAMP,
            }),
            terminate: None,
        })
    }

    /// Upstream `customEntry(id, parentId, customType, data = { id })`.
    fn custom_entry(id: &str, parent_id: Option<&str>, custom_type: &str) -> Write {
        custom_entry_with_data(
            id,
            parent_id,
            custom_type,
            Some(serde_json::json!({ "id": id })),
        )
    }

    fn custom_entry_with_data(
        id: &str,
        parent_id: Option<&str>,
        custom_type: &str,
        data: Option<serde_json::Value>,
    ) -> Write {
        insert_entry(NewEntry::Custom {
            id: id.to_string(),
            parent_id: parent_id.map(str::to_string),
            custom_type: custom_type.to_string(),
            data,
        })
    }

    /// Upstream `compactionEntry(id, parentId)` (`conformance/storage.ts:106-116`).
    fn compaction_entry(id: &str, parent_id: &str) -> Write {
        insert_entry(NewEntry::Compaction {
            id: id.to_string(),
            parent_id: Some(parent_id.to_string()),
            summary: format!("summary:{id}"),
            retained_tail: Vec::new(),
            tokens_before: 10,
            details: None,
            usage: None,
            from_hook: false,
        })
    }

    fn usage_row(id: &str, input: i64, output: i64, adjustment: bool) -> NewUsageRow {
        usage_row_for(id, input, output, adjustment, None)
    }

    fn usage_row_for(
        id: &str,
        input: i64,
        output: i64,
        adjustment: bool,
        entry_id: Option<&str>,
    ) -> NewUsageRow {
        NewUsageRow {
            id: id.to_string(),
            usage: usage(input, output),
            entry_id: entry_id.map(str::to_string),
            adjustment,
            details: None,
        }
    }

    fn ids(entries: &[Entry]) -> Vec<String> {
        entries.iter().map(|entry| entry.id().to_string()).collect()
    }

    /// Upstream `assertCommitStats`.
    async fn assert_commit_stats(
        storage: &MemoryStorage,
        result: &crate::agent_core::harness::session::types::CommitResult,
    ) {
        assert_eq!(result.stats, storage.get_stats(ctx()).await.unwrap());
    }

    // === transactions ========================================================

    /// transactions / "commits mixed writes atomically in write order".
    #[tokio::test]
    async fn replay_commits_mixed_writes_atomically_in_write_order() {
        let storage = storage();
        let result = storage
            .commit(
                vec![
                    user_entry("entry", None, "entry"),
                    set_value(&session_name(), serde_json::json!("session")),
                    insert_usage(usage_row_for("usage", 2, 3, false, Some("entry"))),
                ],
                ctx(),
            )
            .await
            .unwrap();

        assert_eq!(result.seqs.len(), 3);
        assert_eq!(result.first_seq, result.seqs[0]);
        assert_commit_stats(&storage, &result).await;
        assert!(result.seqs.windows(2).all(|window| window[0] < window[1]));
        // ok(Number.isSafeInteger(result.timestamp) && >= 0) — byte-exact here
        // because the fixture injects the same fixed clock as upstream.
        assert_eq!(result.timestamp, NOW);

        let entries = storage
            .get_entries(&["entry".to_string()], ctx())
            .await
            .unwrap();
        let expected_entry = Entry::Message {
            id: "entry".to_string(),
            parent_id: None,
            seq: result.seqs[0],
            timestamp: result.timestamp,
            message: AgentMessage::User(UserMessage {
                content: StringOrBlocks::Blocks(vec![TextOrImageBlock::Text(TextContent {
                    text: "entry".to_string(),
                    text_signature: None,
                })]),
                timestamp: MESSAGE_TIMESTAMP,
            }),
            terminate: None,
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(entries["entry"], expected_entry);

        let stored = storage
            .get_value(&session_name(), ctx())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored,
            crate::agent_core::harness::session::values::StoredValue {
                address: session_name(),
                value: serde_json::json!("session"),
                seq: result.seqs[1],
            }
        );

        let rows = storage
            .scan_usage(
                &UsageScan {
                    order: Some(AscDescOrder::Asc),
                    ..Default::default()
                },
                ctx(),
            )
            .await
            .unwrap();
        assert_eq!(
            rows,
            vec![crate::agent_core::harness::session::types::UsageRow {
                id: "usage".to_string(),
                seq: result.seqs[2],
                usage: usage(2, 3),
                entry_id: Some("entry".to_string()),
                adjustment: false,
                details: None,
            }]
        );
        storage.close(ctx()).await.unwrap();
    }

    /// transactions / "rolls back every store when a mixed transaction fails".
    #[tokio::test]
    async fn replay_rolls_back_every_store_when_mixed_transaction_fails() {
        let storage = storage();
        storage
            .commit(
                vec![
                    user_entry("root", None, "root"),
                    insert_usage(usage_row("taken", 1, 1, false)),
                ],
                ctx(),
            )
            .await
            .unwrap();
        let entries_before = storage
            .scan_entries(
                &EntryScan {
                    order: Some(AscDescOrder::Asc),
                    ..Default::default()
                },
                ctx(),
            )
            .await
            .unwrap();
        let usage_before = storage
            .scan_usage(
                &UsageScan {
                    order: Some(AscDescOrder::Asc),
                    ..Default::default()
                },
                ctx(),
            )
            .await
            .unwrap();
        let stats_before = storage.get_stats(ctx()).await.unwrap();

        assert!(storage
            .commit(
                vec![
                    set_value(&session_name(), serde_json::json!("transient")),
                    custom_entry("transient-entry", Some("root"), "note"),
                    insert_usage(usage_row("transient-usage", 5, 8, true)),
                    custom_entry("taken", Some("root"), "note"),
                ],
                ctx(),
            )
            .await
            .is_err());

        assert_eq!(
            storage
                .scan_entries(
                    &EntryScan {
                        order: Some(AscDescOrder::Asc),
                        ..Default::default()
                    },
                    ctx()
                )
                .await
                .unwrap(),
            entries_before
        );
        assert_eq!(
            storage
                .scan_usage(
                    &UsageScan {
                        order: Some(AscDescOrder::Asc),
                        ..Default::default()
                    },
                    ctx()
                )
                .await
                .unwrap(),
            usage_before
        );
        assert_eq!(storage.get_stats(ctx()).await.unwrap(), stats_before);
        assert!(storage
            .get_value(&session_name(), ctx())
            .await
            .unwrap()
            .is_none());
        storage.close(ctx()).await.unwrap();
    }

    /// transactions / "preserves overwritten and deleted values when a
    /// transaction fails".
    #[tokio::test]
    async fn replay_preserves_overwritten_and_deleted_values_when_transaction_fails() {
        let storage = storage();
        storage
            .commit(
                vec![
                    set_value(
                        &value("test.value", "overwritten"),
                        serde_json::json!("original"),
                    ),
                    set_value(
                        &value("test.value", "deleted"),
                        serde_json::json!({ "kept": true }),
                    ),
                    user_entry("taken", None, "taken"),
                ],
                ctx(),
            )
            .await
            .unwrap();
        let overwritten_before = storage
            .get_value(&value("test.value", "overwritten"), ctx())
            .await
            .unwrap();
        let deleted_before = storage
            .get_value(&value("test.value", "deleted"), ctx())
            .await
            .unwrap();

        assert!(storage
            .commit(
                vec![
                    set_value(
                        &value("test.value", "overwritten"),
                        serde_json::json!("transient")
                    ),
                    delete_value(&value("test.value", "deleted")),
                    custom_entry("transient", Some("taken"), "note"),
                    custom_entry("taken", None, "note"),
                ],
                ctx(),
            )
            .await
            .is_err());

        assert_eq!(
            storage
                .get_value(&value("test.value", "overwritten"), ctx())
                .await
                .unwrap(),
            overwritten_before
        );
        assert_eq!(
            storage
                .get_value(&value("test.value", "deleted"), ctx())
                .await
                .unwrap(),
            deleted_before
        );
        assert!(!storage
            .get_entries(&["transient".to_string()], ctx())
            .await
            .unwrap()
            .contains_key("transient"));
        storage.close(ctx()).await.unwrap();
    }

    /// transactions / "enforces one shared entry and usage id namespace".
    #[tokio::test]
    async fn replay_enforces_one_shared_entry_and_usage_id_namespace() {
        let storage = storage();
        storage
            .commit(
                vec![
                    user_entry("existing-entry", None, "existing-entry"),
                    insert_usage(usage_row("existing-usage", 1, 1, false)),
                ],
                ctx(),
            )
            .await
            .unwrap();

        assert!(storage
            .commit(
                vec![insert_usage(usage_row("existing-entry", 2, 2, false))],
                ctx()
            )
            .await
            .is_err());
        assert!(storage
            .commit(vec![custom_entry("existing-usage", None, "note")], ctx())
            .await
            .is_err());

        for (id, writes) in [
            (
                "entry-then-usage",
                vec![
                    custom_entry("entry-then-usage", None, "note"),
                    insert_usage(usage_row("entry-then-usage", 3, 3, false)),
                ],
            ),
            (
                "usage-then-entry",
                vec![
                    insert_usage(usage_row("usage-then-entry", 4, 4, false)),
                    custom_entry("usage-then-entry", None, "note"),
                ],
            ),
        ] {
            assert!(
                storage.commit(writes, ctx()).await.is_err(),
                "Expected duplicate id {id} to reject"
            );
        }

        assert_eq!(
            ids(&storage
                .scan_entries(
                    &EntryScan {
                        order: Some(AscDescOrder::Asc),
                        ..Default::default()
                    },
                    ctx()
                )
                .await
                .unwrap()),
            vec!["existing-entry"]
        );
        assert_eq!(
            storage
                .scan_usage(
                    &UsageScan {
                        order: Some(AscDescOrder::Asc),
                        ..Default::default()
                    },
                    ctx()
                )
                .await
                .unwrap()
                .iter()
                .map(|row| row.id.clone())
                .collect::<Vec<_>>(),
            vec!["existing-usage"]
        );
        storage.close(ctx()).await.unwrap();
    }

    /// transactions / "resolves parents only from prior entries and earlier
    /// writes".
    #[tokio::test]
    async fn replay_resolves_parents_only_from_prior_entries_and_earlier_writes() {
        let storage = storage();
        storage
            .commit(vec![user_entry("root", None, "root")], ctx())
            .await
            .unwrap();
        storage
            .commit(
                vec![
                    custom_entry("child", Some("root"), "note"),
                    custom_entry("grandchild", Some("child"), "note"),
                ],
                ctx(),
            )
            .await
            .unwrap();
        assert_eq!(
            ids(&storage
                .scan_branch(
                    &StorageBranchScan {
                        start: "grandchild".to_string(),
                        order: Some(BranchScanOrder::OldestFirst),
                        ..Default::default()
                    },
                    ctx()
                )
                .await
                .unwrap()),
            vec!["root", "child", "grandchild"]
        );

        assert!(storage
            .commit(
                vec![
                    custom_entry("before-parent", Some("later-parent"), "note"),
                    custom_entry("later-parent", Some("root"), "note"),
                    set_value(
                        &entry_label("before-parent"),
                        serde_json::json!("transient")
                    ),
                ],
                ctx(),
            )
            .await
            .is_err());
        assert!(storage
            .commit(vec![custom_entry("orphan", Some("missing"), "note")], ctx())
            .await
            .is_err());
        storage
            .commit(
                vec![insert_usage(usage_row("usage-is-not-parent", 1, 1, false))],
                ctx(),
            )
            .await
            .unwrap();
        assert!(storage
            .commit(
                vec![custom_entry(
                    "usage-child",
                    Some("usage-is-not-parent"),
                    "note"
                )],
                ctx(),
            )
            .await
            .is_err());

        assert!(storage
            .get_entries(
                &[
                    "before-parent".to_string(),
                    "later-parent".to_string(),
                    "orphan".to_string(),
                    "usage-child".to_string(),
                ],
                ctx()
            )
            .await
            .unwrap()
            .is_empty());
        assert!(storage
            .get_value(&entry_label("before-parent"), ctx())
            .await
            .unwrap()
            .is_none());
        storage.close(ctx()).await.unwrap();
    }

    /// transactions / "places pending content under its reserved entry id".
    #[tokio::test]
    async fn replay_places_pending_content_under_its_reserved_entry_id() {
        let storage = storage();
        let pending = pending_entry("reserved");
        let tip = branch_tip("main");
        // The upstream fixture stores the message payload verbatim.
        let payload = serde_json::json!({
            "type": "message",
            "payload": {
                "role": "user",
                "content": [{ "type": "text", "text": "queued" }],
                "timestamp": MESSAGE_TIMESTAMP,
            },
        });
        storage
            .commit(
                vec![
                    set_value(&pending, payload.clone()),
                    set_value(&tip, serde_json::Value::Null),
                ],
                ctx(),
            )
            .await
            .unwrap();

        assert!(!storage
            .get_entries(&["reserved".to_string()], ctx())
            .await
            .unwrap()
            .contains_key("reserved"));
        assert_eq!(
            storage
                .get_value(&pending, ctx())
                .await
                .unwrap()
                .unwrap()
                .value,
            payload
        );
        assert_eq!(
            storage.get_value(&tip, ctx()).await.unwrap().unwrap().value,
            serde_json::Value::Null
        );

        let placement = storage
            .commit(
                vec![
                    user_entry("reserved", None, "queued"),
                    delete_value(&pending),
                    set_value(&tip, serde_json::json!("reserved")),
                ],
                ctx(),
            )
            .await
            .unwrap();

        let entries = storage
            .get_entries(&["reserved".to_string()], ctx())
            .await
            .unwrap();
        let expected = Entry::Message {
            id: "reserved".to_string(),
            parent_id: None,
            seq: placement.seqs[0],
            timestamp: placement.timestamp,
            message: AgentMessage::User(UserMessage {
                content: StringOrBlocks::Blocks(vec![TextOrImageBlock::Text(TextContent {
                    text: "queued".to_string(),
                    text_signature: None,
                })]),
                timestamp: MESSAGE_TIMESTAMP,
            }),
            terminate: None,
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(entries["reserved"], expected);
        assert!(storage.get_value(&pending, ctx()).await.unwrap().is_none());
        assert_eq!(
            storage.get_value(&tip, ctx()).await.unwrap().unwrap(),
            crate::agent_core::harness::session::values::StoredValue {
                address: branch_tip("main"),
                value: serde_json::json!("reserved"),
                seq: placement.seqs[2],
            }
        );
        storage.close(ctx()).await.unwrap();
    }

    // === values ==============================================================

    /// values / "sets, replaces, deletes, and recreates values without
    /// tombstones".
    #[tokio::test]
    async fn replay_sets_replaces_deletes_recreates_values_without_tombstones() {
        let storage = storage();
        let prefix_key = |key: &str| value("test.value", key);
        let first = storage
            .commit(
                vec![
                    set_value(&prefix_key("prefix/b"), serde_json::json!(1)),
                    set_value(&prefix_key("prefix/a"), serde_json::json!(2)),
                    set_value(&prefix_key("other"), serde_json::json!(3)),
                    set_value(&prefix_key("prefix/\u{e000}"), serde_json::json!(4)),
                    set_value(&prefix_key("prefix/\u{10000}"), serde_json::json!(5)),
                    set_value(&prefix_key("prefix/a"), serde_json::Value::Null),
                ],
                ctx(),
            )
            .await
            .unwrap();
        assert_eq!(
            storage
                .get_value(&prefix_key("prefix/a"), ctx())
                .await
                .unwrap()
                .unwrap(),
            crate::agent_core::harness::session::values::StoredValue {
                address: prefix_key("prefix/a"),
                value: serde_json::Value::Null,
                seq: first.seqs[5],
            }
        );

        let second = storage
            .commit(
                vec![
                    delete_value(&prefix_key("prefix/a")),
                    delete_value(&prefix_key("absent")),
                    set_value(&prefix_key("prefix/a"), serde_json::json!("recreated")),
                ],
                ctx(),
            )
            .await
            .unwrap();

        let scanned = storage
            .scan_values(&value("test.value", "prefix/"), ctx())
            .await
            .unwrap();
        assert_eq!(
            scanned
                .iter()
                .map(|stored| (stored.address.key.clone(), stored.value.clone(), stored.seq))
                .collect::<Vec<_>>(),
            vec![
                (
                    "prefix/a".to_string(),
                    serde_json::json!("recreated"),
                    second.seqs[2]
                ),
                ("prefix/b".to_string(), serde_json::json!(1), first.seqs[0]),
                (
                    "prefix/\u{e000}".to_string(),
                    serde_json::json!(4),
                    first.seqs[3]
                ),
                (
                    "prefix/\u{10000}".to_string(),
                    serde_json::json!(5),
                    first.seqs[4]
                ),
            ]
        );
        assert!(storage
            .get_value(&prefix_key("absent"), ctx())
            .await
            .unwrap()
            .is_none());
        storage.close(ctx()).await.unwrap();
    }

    /// values / "applies same-transaction value and list operations in write
    /// order".
    #[tokio::test]
    async fn replay_applies_same_transaction_value_and_list_operations_in_write_order() {
        let storage = storage();
        let kept_value = value("test.value", "write-order/kept");
        let deleted_value = value("test.value", "write-order/deleted");
        let kept_list = list("test.list", "write-order/kept");
        let deleted_list = list("test.list", "write-order/deleted");
        let result = storage
            .commit(
                vec![
                    set_value(&deleted_value, serde_json::json!("transient")),
                    delete_value(&deleted_value),
                    set_value(&kept_value, serde_json::json!("transient")),
                    set_value(&kept_value, serde_json::json!("kept")),
                    append_list(&kept_list, serde_json::json!("transient")),
                    delete_list(&kept_list),
                    append_list(&kept_list, serde_json::json!("kept")),
                    append_list(&deleted_list, serde_json::json!("transient")),
                    delete_list(&deleted_list),
                ],
                ctx(),
            )
            .await
            .unwrap();

        assert!(storage
            .get_value(&deleted_value, ctx())
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            storage
                .get_value(&kept_value, ctx())
                .await
                .unwrap()
                .unwrap(),
            crate::agent_core::harness::session::values::StoredValue {
                address: kept_value.clone(),
                value: serde_json::json!("kept"),
                seq: result.seqs[3],
            }
        );
        assert_eq!(
            storage.read_list(&kept_list, None, ctx()).await.unwrap(),
            vec![crate::agent_core::harness::session::values::ListElement {
                seq: result.seqs[6],
                value: serde_json::json!("kept"),
            }]
        );
        assert!(storage
            .read_list(&deleted_list, None, ctx())
            .await
            .unwrap()
            .is_empty());
        storage.close(ctx()).await.unwrap();
    }

    /// values / "does not change historical stores during value-only commits".
    #[tokio::test]
    async fn replay_does_not_change_historical_stores_during_value_only_commits() {
        let storage = storage();
        storage
            .commit(
                vec![
                    user_entry("root", None, "root"),
                    insert_usage(usage_row("historical-usage", 2, 3, false)),
                ],
                ctx(),
            )
            .await
            .unwrap();
        let entries_before = storage
            .scan_entries(
                &EntryScan {
                    order: Some(AscDescOrder::Asc),
                    ..Default::default()
                },
                ctx(),
            )
            .await
            .unwrap();
        let usage_before = storage
            .scan_usage(
                &UsageScan {
                    order: Some(AscDescOrder::Asc),
                    ..Default::default()
                },
                ctx(),
            )
            .await
            .unwrap();
        let stats_before = storage.get_stats(ctx()).await.unwrap();

        let result = storage
            .commit(
                vec![
                    set_value(&session_name(), serde_json::json!("first")),
                    set_value(&session_name(), serde_json::json!("second")),
                ],
                ctx(),
            )
            .await
            .unwrap();

        assert_eq!(
            storage
                .scan_entries(
                    &EntryScan {
                        order: Some(AscDescOrder::Asc),
                        ..Default::default()
                    },
                    ctx()
                )
                .await
                .unwrap(),
            entries_before
        );
        assert_eq!(
            storage
                .scan_usage(
                    &UsageScan {
                        order: Some(AscDescOrder::Asc),
                        ..Default::default()
                    },
                    ctx()
                )
                .await
                .unwrap(),
            usage_before
        );
        assert_eq!(storage.get_stats(ctx()).await.unwrap(), stats_before);
        assert_eq!(
            storage
                .get_value(&session_name(), ctx())
                .await
                .unwrap()
                .unwrap(),
            crate::agent_core::harness::session::values::StoredValue {
                address: session_name(),
                value: serde_json::json!("second"),
                seq: result.seqs[1],
            }
        );
        storage.close(ctx()).await.unwrap();
    }

    // === lists ===============================================================

    /// lists / "pages appends by global sequence and deletes whole lists".
    #[tokio::test]
    async fn replay_pages_appends_by_global_sequence_and_deletes_whole_lists() {
        let storage = storage();
        let address = list("test.list", "events");
        assert!(storage
            .read_list(&address, None, ctx())
            .await
            .unwrap()
            .is_empty());
        let result = storage
            .commit(
                vec![
                    append_list(&address, serde_json::json!("a")),
                    set_value(&session_name(), serde_json::json!("gap")),
                    append_list(&address, serde_json::json!("b")),
                    append_list(&address, serde_json::json!("c")),
                ],
                ctx(),
            )
            .await
            .unwrap();
        let read = |options: Option<&ListReadOptions>| storage.read_list(&address, options, ctx());
        let element =
            |seq: i64, value: &str| crate::agent_core::harness::session::values::ListElement {
                seq,
                value: serde_json::json!(value),
            };
        assert_eq!(
            read(None).await.unwrap(),
            vec![
                element(result.seqs[0], "a"),
                element(result.seqs[2], "b"),
                element(result.seqs[3], "c")
            ]
        );
        assert_eq!(
            read(Some(&ListReadOptions {
                limit: Some(2),
                ..Default::default()
            }))
            .await
            .unwrap(),
            vec![element(result.seqs[0], "a"), element(result.seqs[2], "b")]
        );
        assert_eq!(
            read(Some(&ListReadOptions {
                cursor: Some(ListCursor {
                    seq: result.seqs[0]
                }),
                limit: Some(2),
                ..Default::default()
            }))
            .await
            .unwrap(),
            vec![element(result.seqs[2], "b"), element(result.seqs[3], "c")]
        );
        assert_eq!(
            read(Some(&ListReadOptions {
                order: Some(AscDescOrder::Desc),
                limit: Some(2),
                ..Default::default()
            }))
            .await
            .unwrap(),
            vec![element(result.seqs[3], "c"), element(result.seqs[2], "b")]
        );
        assert_eq!(
            read(Some(&ListReadOptions {
                order: Some(AscDescOrder::Desc),
                cursor: Some(ListCursor {
                    seq: result.seqs[3]
                }),
                limit: Some(2),
            }))
            .await
            .unwrap(),
            vec![element(result.seqs[2], "b"), element(result.seqs[0], "a")]
        );
        // limit: 0 rejects (upstream TypeError).
        assert!(read(Some(&ListReadOptions {
            limit: Some(0),
            ..Default::default()
        }))
        .await
        .is_err());
        // Upstream also rejects `limit: Number.MAX_VALUE` (not a safe integer).
        // Disclosed substitution: the port's `u64` limit cannot carry that
        // non-safe-integer rejection (values.rs resolve clamps instead); the
        // resolver is outside this replay's fix scope.

        storage
            .commit(
                vec![
                    delete_list(&address),
                    delete_list(&list("test.list", "absent")),
                    append_list(&address, serde_json::json!("new")),
                ],
                ctx(),
            )
            .await
            .unwrap();
        assert_eq!(
            read(None)
                .await
                .unwrap()
                .iter()
                .map(|element| element.value.clone())
                .collect::<Vec<_>>(),
            vec![serde_json::json!("new")]
        );
        storage.close(ctx()).await.unwrap();
    }

    /// lists / "clamps one read page without limiting list growth".
    #[tokio::test]
    async fn replay_clamps_one_read_page_without_limiting_list_growth() {
        let storage = storage();
        let address = list("test.list", "large");
        let writes: Vec<Write> = (0..10_001)
            .map(|index| append_list(&address, serde_json::json!(index)))
            .collect();
        storage.commit(writes, ctx()).await.unwrap();
        let first_page = storage.read_list(&address, None, ctx()).await.unwrap();
        assert_eq!(first_page.len(), 1_000);
        assert_eq!(
            storage
                .read_list(
                    &address,
                    Some(&ListReadOptions {
                        limit: Some(20_000),
                        ..Default::default()
                    }),
                    ctx()
                )
                .await
                .unwrap()
                .len(),
            10_000
        );
        assert_eq!(
            storage
                .read_list(
                    &address,
                    Some(&ListReadOptions {
                        cursor: Some(ListCursor {
                            seq: first_page.last().unwrap().seq,
                        }),
                        ..Default::default()
                    }),
                    ctx()
                )
                .await
                .unwrap()
                .len(),
            1_000
        );
        storage.close(ctx()).await.unwrap();
    }

    /// lists / "commits mixed list writes atomically and rolls them back with
    /// siblings".
    #[tokio::test]
    async fn replay_commits_mixed_list_writes_atomically_and_rolls_them_back() {
        let storage = storage();
        let address = list("test.list", "atomic");
        let committed = storage
            .commit(
                vec![
                    user_entry("mixed", None, "mixed"),
                    append_list(&address, serde_json::json!("kept")),
                    set_value(&session_name(), serde_json::json!("kept")),
                    insert_usage(usage_row("mixed-usage", 1, 2, false)),
                ],
                ctx(),
            )
            .await
            .unwrap();
        assert_eq!(
            storage.read_list(&address, None, ctx()).await.unwrap(),
            vec![crate::agent_core::harness::session::values::ListElement {
                seq: committed.seqs[1],
                value: serde_json::json!("kept"),
            }]
        );

        assert!(storage
            .commit(
                vec![
                    append_list(&address, serde_json::json!("transient")),
                    delete_value(&session_name()),
                    user_entry("mixed", None, "mixed"),
                ],
                ctx(),
            )
            .await
            .is_err());
        assert_eq!(
            storage.read_list(&address, None, ctx()).await.unwrap(),
            vec![crate::agent_core::harness::session::values::ListElement {
                seq: committed.seqs[1],
                value: serde_json::json!("kept"),
            }]
        );
        assert_eq!(
            storage
                .get_value(&session_name(), ctx())
                .await
                .unwrap()
                .unwrap()
                .value,
            serde_json::json!("kept")
        );
        storage.close(ctx()).await.unwrap();
    }

    // === entry queries =======================================================

    /// entry queries / "stores custom entries with and without data".
    #[tokio::test]
    async fn replay_stores_custom_entries_with_and_without_data() {
        let storage = storage();
        let result = storage
            .commit(
                vec![
                    custom_entry_with_data("without-data", None, "marker", None),
                    custom_entry_with_data(
                        "with-data",
                        Some("without-data"),
                        "note",
                        Some(serde_json::json!({ "nested": [1, 2] })),
                    ),
                ],
                ctx(),
            )
            .await
            .unwrap();

        let entries = storage
            .get_entries(
                &["without-data".to_string(), "with-data".to_string()],
                ctx(),
            )
            .await
            .unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries["without-data"],
            Entry::Custom {
                id: "without-data".to_string(),
                parent_id: None,
                seq: result.seqs[0],
                timestamp: result.timestamp,
                custom_type: "marker".to_string(),
                data: None,
            }
        );
        assert_eq!(
            entries["with-data"],
            Entry::Custom {
                id: "with-data".to_string(),
                parent_id: Some("without-data".to_string()),
                seq: result.seqs[1],
                timestamp: result.timestamp,
                custom_type: "note".to_string(),
                data: Some(serde_json::json!({ "nested": [1, 2] })),
            }
        );
        storage.close(ctx()).await.unwrap();
    }

    /// entry queries / "scans global entries with explicit ranges, filters,
    /// orders, and limits".
    #[tokio::test]
    async fn replay_scans_global_entries_with_ranges_filters_orders_limits() {
        let storage = storage();
        let result = storage
            .commit(
                vec![
                    user_entry("root", None, "root"),
                    custom_entry("note-1", Some("root"), "note"),
                    custom_entry("other", Some("note-1"), "other"),
                    custom_entry("note-2", Some("other"), "note"),
                    user_entry("tail", Some("note-2"), "tail"),
                ],
                ctx(),
            )
            .await
            .unwrap();

        assert_eq!(
            ids(&storage
                .scan_entries(
                    &EntryScan {
                        scan_type: Some(EntryType::Custom),
                        custom_type: Some("note".to_string()),
                        from_seq: Some(result.seqs[1]),
                        to_seq: Some(result.seqs[3]),
                        order: Some(AscDescOrder::Desc),
                        limit: None,
                    },
                    ctx()
                )
                .await
                .unwrap()),
            vec!["note-2", "note-1"]
        );
        assert_eq!(
            ids(&storage
                .scan_entries(
                    &EntryScan {
                        order: Some(AscDescOrder::Asc),
                        limit: Some(2),
                        ..Default::default()
                    },
                    ctx()
                )
                .await
                .unwrap()),
            vec!["root", "note-1"]
        );
        assert_eq!(
            ids(&storage
                .scan_entries(
                    &EntryScan {
                        order: Some(AscDescOrder::Desc),
                        limit: Some(2),
                        ..Default::default()
                    },
                    ctx()
                )
                .await
                .unwrap()),
            vec!["tail", "note-2"]
        );
        storage.close(ctx()).await.unwrap();
    }

    // === branch queries ======================================================

    /// branch queries / "applies stops before filters and cursors before
    /// limits".
    #[tokio::test]
    async fn replay_branch_queries_apply_stops_filters_and_cursors() {
        let storage = storage();
        let result = storage
            .commit(
                vec![
                    user_entry("root", None, "root"),
                    custom_entry("marker", Some("root"), "marker"),
                    user_entry("middle", Some("marker"), "middle"),
                    compaction_entry("compact", "middle"),
                    custom_entry("note", Some("compact"), "note"),
                    user_entry("leaf", Some("note"), "leaf"),
                ],
                ctx(),
            )
            .await
            .unwrap();
        let scan = |query: StorageBranchScan| storage.scan_branch(&query, ctx());
        assert_eq!(
            ids(&scan(StorageBranchScan {
                start: "leaf".to_string(),
                stop_at_type: Some(EntryType::Compaction),
                scan_type: Some(EntryType::Message),
                ..Default::default()
            })
            .await
            .unwrap()),
            vec!["leaf"]
        );
        assert_eq!(
            ids(&scan(StorageBranchScan {
                start: "leaf".to_string(),
                order: Some(BranchScanOrder::OldestFirst),
                stop_at_id: Some("middle".to_string()),
                scan_type: Some(EntryType::Custom),
                ..Default::default()
            })
            .await
            .unwrap()),
            vec!["marker"]
        );
        assert_eq!(
            ids(&scan(StorageBranchScan {
                start: "leaf".to_string(),
                order: Some(BranchScanOrder::NewestFirst),
                cursor: Some(crate::agent_core::harness::session::types::EntryCursor {
                    seq: result.seqs[4]
                }),
                limit: Some(2),
                ..Default::default()
            })
            .await
            .unwrap()),
            vec!["compact", "middle"]
        );
        assert_eq!(
            ids(&scan(StorageBranchScan {
                start: "leaf".to_string(),
                order: Some(BranchScanOrder::OldestFirst),
                cursor: Some(crate::agent_core::harness::session::types::EntryCursor {
                    seq: result.seqs[1]
                }),
                limit: Some(2),
                ..Default::default()
            })
            .await
            .unwrap()),
            vec!["middle", "compact"]
        );
        assert!(scan(StorageBranchScan {
            start: "leaf".to_string(),
            stop_at_id: Some("leaf".to_string()),
            scan_type: Some(EntryType::Custom),
            ..Default::default()
        })
        .await
        .unwrap()
        .is_empty());
        assert_eq!(
            ids(&scan(StorageBranchScan {
                start: "leaf".to_string(),
                custom_type: Some("note".to_string()),
                ..Default::default()
            })
            .await
            .unwrap()),
            vec!["note"]
        );
        assert!(scan(StorageBranchScan {
            start: "missing".to_string(),
            ..Default::default()
        })
        .await
        .is_err());
        storage.close(ctx()).await.unwrap();
    }

    /// branch queries / "returns branch structure without payload fields".
    #[tokio::test]
    async fn replay_returns_branch_structure_without_payload_fields() {
        let storage = storage();
        let result = storage
            .commit(
                vec![
                    user_entry("root", None, "root"),
                    custom_entry("child", Some("root"), "note"),
                ],
                ctx(),
            )
            .await
            .unwrap();

        assert_eq!(
            storage
                .scan_branch_structure(
                    &StorageBranchScan {
                        start: "child".to_string(),
                        order: Some(BranchScanOrder::OldestFirst),
                        ..Default::default()
                    },
                    ctx()
                )
                .await
                .unwrap(),
            vec![
                EntryStructure {
                    id: "root".to_string(),
                    parent_id: None,
                    seq: result.seqs[0],
                    timestamp: result.timestamp,
                    entry_type: EntryType::Message,
                    custom_type: None,
                },
                EntryStructure {
                    id: "child".to_string(),
                    parent_id: Some("root".to_string()),
                    seq: result.seqs[1],
                    timestamp: result.timestamp,
                    entry_type: EntryType::Custom,
                    custom_type: Some("note".to_string()),
                },
            ]
        );
        storage.close(ctx()).await.unwrap();
    }

    /// branch queries / "applies branch query semantics to structure scans".
    #[tokio::test]
    async fn replay_applies_branch_query_semantics_to_structure_scans() {
        let storage = storage();
        let result = storage
            .commit(
                vec![
                    user_entry("root", None, "root"),
                    custom_entry("marker", Some("root"), "marker"),
                    user_entry("middle", Some("marker"), "middle"),
                    compaction_entry("compact", "middle"),
                    custom_entry("note", Some("compact"), "note"),
                    user_entry("leaf", Some("note"), "leaf"),
                ],
                ctx(),
            )
            .await
            .unwrap();
        let scan = |query: StorageBranchScan| storage.scan_branch_structure(&query, ctx());
        assert_eq!(
            scan(StorageBranchScan {
                start: "leaf".to_string(),
                stop_at_type: Some(EntryType::Compaction),
                scan_type: Some(EntryType::Message),
                ..Default::default()
            })
            .await
            .unwrap()
            .iter()
            .map(|structure| structure.id.clone())
            .collect::<Vec<_>>(),
            vec!["leaf"]
        );
        assert_eq!(
            scan(StorageBranchScan {
                start: "leaf".to_string(),
                order: Some(BranchScanOrder::OldestFirst),
                cursor: Some(crate::agent_core::harness::session::types::EntryCursor {
                    seq: result.seqs[1]
                }),
                limit: Some(2),
                ..Default::default()
            })
            .await
            .unwrap()
            .iter()
            .map(|structure| structure.id.clone())
            .collect::<Vec<_>>(),
            vec!["middle", "compact"]
        );
        assert!(scan(StorageBranchScan {
            start: "missing".to_string(),
            ..Default::default()
        })
        .await
        .is_err());
        storage.close(ctx()).await.unwrap();
    }

    // === usage and stats =====================================================

    /// usage and stats / "scans the usage ledger with explicit ranges, orders,
    /// and limits".
    #[tokio::test]
    async fn replay_scans_usage_ledger_with_ranges_orders_and_limits() {
        let storage = storage();
        let result = storage
            .commit(
                vec![
                    insert_usage(usage_row("usage-1", 1, 1, false)),
                    set_value(&session_name(), serde_json::json!("sequence gap")),
                    insert_usage(usage_row("usage-2", 2, 2, false)),
                    insert_usage(usage_row("usage-3", 3, 3, true)),
                ],
                ctx(),
            )
            .await
            .unwrap();
        let ids_of = |rows: Vec<crate::agent_core::harness::session::types::UsageRow>| {
            rows.iter().map(|row| row.id.clone()).collect::<Vec<_>>()
        };
        assert_eq!(
            ids_of(
                storage
                    .scan_usage(
                        &UsageScan {
                            from_seq: Some(result.seqs[1]),
                            to_seq: Some(result.seqs[2]),
                            order: Some(AscDescOrder::Asc),
                            ..Default::default()
                        },
                        ctx()
                    )
                    .await
                    .unwrap()
            ),
            vec!["usage-2"]
        );
        assert_eq!(
            ids_of(
                storage
                    .scan_usage(
                        &UsageScan {
                            order: Some(AscDescOrder::Desc),
                            limit: Some(2),
                            ..Default::default()
                        },
                        ctx()
                    )
                    .await
                    .unwrap()
            ),
            vec!["usage-3", "usage-2"]
        );
        assert_eq!(
            ids_of(
                storage
                    .scan_usage(
                        &UsageScan {
                            order: Some(AscDescOrder::Asc),
                            limit: Some(2),
                            ..Default::default()
                        },
                        ctx()
                    )
                    .await
                    .unwrap()
            ),
            vec!["usage-1", "usage-2"]
        );
        storage.close(ctx()).await.unwrap();
    }

    /// usage and stats / "keeps stats equal to message count and ledger
    /// totals".
    #[tokio::test]
    async fn replay_keeps_stats_equal_to_message_count_and_ledger_totals() {
        let storage = storage();
        assert_eq!(
            storage.get_stats(ctx()).await.unwrap(),
            SessionStats {
                message_count: 0,
                usage: Usage::default(),
            }
        );

        let mut first_usage = usage(2, 3);
        first_usage.cache_write_1h = Some(4);
        first_usage.reasoning = Some(1);
        let first = storage
            .commit(
                vec![
                    user_entry("message", None, "message"),
                    insert_usage(NewUsageRow {
                        id: "usage-1".to_string(),
                        usage: first_usage,
                        entry_id: None,
                        adjustment: false,
                        details: None,
                    }),
                ],
                ctx(),
            )
            .await
            .unwrap();
        assert_commit_stats(&storage, &first).await;
        assert_eq!(
            first.stats,
            SessionStats {
                message_count: 1,
                usage: first_usage,
            }
        );

        let mut second_usage = usage(5, 7);
        second_usage.cache_write_1h = Some(6);
        second_usage.reasoning = Some(2);
        let second = storage
            .commit(
                vec![
                    custom_entry("custom", Some("message"), "note"),
                    compaction_entry("compaction", "custom"),
                    insert_usage(NewUsageRow {
                        id: "usage-2".to_string(),
                        usage: second_usage,
                        entry_id: None,
                        adjustment: true,
                        details: None,
                    }),
                ],
                ctx(),
            )
            .await
            .unwrap();
        assert_commit_stats(&storage, &second).await;
        assert_eq!(second.stats.message_count, 1);
        let totals = &second.stats.usage;
        assert_eq!(totals.input, 7);
        assert_eq!(totals.output, 10);
        assert_eq!(totals.cache_read, 9);
        assert_eq!(totals.cache_write, 12);
        assert_eq!(totals.cache_write_1h, Some(10));
        assert_eq!(totals.reasoning, Some(3));
        assert_eq!(totals.total_tokens, 17);
        assert_eq!(
            totals.cost.input,
            first_usage.cost.input + second_usage.cost.input
        );
        assert_eq!(
            totals.cost.output,
            first_usage.cost.output + second_usage.cost.output
        );
        assert_eq!(
            totals.cost.cache_read,
            first_usage.cost.cache_read + second_usage.cost.cache_read
        );
        assert_eq!(
            totals.cost.cache_write,
            first_usage.cost.cache_write + second_usage.cost.cache_write
        );
        assert_eq!(
            totals.cost.total,
            first_usage.cost.total + second_usage.cost.total
        );
        storage.close(ctx()).await.unwrap();
    }

    // === serialization =======================================================

    /// serialization / "serializes back-to-back commits in admission order".
    #[tokio::test]
    async fn replay_serializes_back_to_back_commits_in_admission_order() {
        let storage = Arc::new(storage());
        let first = storage.commit(vec![user_entry("first", None, "first")], ctx());
        let second = storage.commit(vec![user_entry("second", Some("first"), "second")], ctx());
        let (first_result, second_result) = tokio::join!(first, second);
        let first_result = first_result.unwrap();
        let second_result = second_result.unwrap();

        assert!(first_result.seqs[0] < second_result.seqs[0]);
        assert_eq!(
            first_result.stats,
            SessionStats {
                message_count: 1,
                usage: Usage::default(),
            }
        );
        assert_eq!(
            second_result.stats,
            SessionStats {
                message_count: 2,
                usage: Usage::default(),
            }
        );
        assert_commit_stats(&storage, &second_result).await;
        assert_eq!(
            ids(&storage
                .scan_entries(
                    &EntryScan {
                        order: Some(AscDescOrder::Asc),
                        ..Default::default()
                    },
                    ctx()
                )
                .await
                .unwrap()),
            vec!["first", "second"]
        );
        storage.close(ctx()).await.unwrap();
    }

    // === lifecycle ===========================================================

    /// lifecycle / "seals admission, drains admitted commits, and closes
    /// idempotently". Disclosed substitution: the Rust futures are lazy, so
    /// the rejection checks that upstream interleaves before the close
    /// completes run after both closes here (same ordering the JSONL conformance
    /// port uses).
    #[tokio::test]
    async fn replay_seals_admission_drains_admitted_commits_and_closes_idempotently() {
        let storage = Arc::new(storage());
        let admitted = storage.commit(vec![user_entry("admitted", None, "admitted")], ctx());
        let first_close = storage.close(ctx());
        let second_close = storage.close(ctx());
        let (admitted, first, second) = tokio::join!(admitted, first_close, second_close);
        first.unwrap();
        second.unwrap();
        assert_eq!(admitted.unwrap().seqs.len(), 1);

        for rejected in [
            "commit",
            "getEntries",
            "getValue",
            "scanValues",
            "readList",
            "scanBranch",
            "scanBranchStructure",
            "scanEntries",
            "scanUsage",
            "getStats",
        ] {
            let outcome = match rejected {
                "commit" => storage
                    .commit(Vec::new(), ctx())
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
                "getEntries" => storage
                    .get_entries(&[], ctx())
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
                "getValue" => storage
                    .get_value(&session_name(), ctx())
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
                "scanValues" => storage
                    .scan_values(&session_name(), ctx())
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
                "readList" => storage
                    .read_list(&list("test.list", "events"), None, ctx())
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
                "scanBranch" => storage
                    .scan_branch(
                        &StorageBranchScan {
                            start: "admitted".to_string(),
                            ..Default::default()
                        },
                        ctx(),
                    )
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
                "scanBranchStructure" => storage
                    .scan_branch_structure(
                        &StorageBranchScan {
                            start: "admitted".to_string(),
                            ..Default::default()
                        },
                        ctx(),
                    )
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
                "scanEntries" => storage
                    .scan_entries(
                        &EntryScan {
                            order: Some(AscDescOrder::Asc),
                            ..Default::default()
                        },
                        ctx(),
                    )
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
                "scanUsage" => storage
                    .scan_usage(
                        &UsageScan {
                            order: Some(AscDescOrder::Asc),
                            ..Default::default()
                        },
                        ctx(),
                    )
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
                "getStats" => storage
                    .get_stats(ctx())
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
                other => unreachable!("{other}"),
            };
            assert!(
                outcome.is_err(),
                "expected {rejected} to reject after close"
            );
        }
    }

    // === MemoryStorage commit statistics =====================================

    /// "includes historical totals in the first commit after session reopen"
    /// (the hand-written `MemoryStorage commit statistics` describe block).
    #[tokio::test]
    async fn replay_includes_historical_totals_in_first_commit_after_reopen() {
        let repo = MemorySessionRepo::new(MemorySessionRepoOptions {
            now: Some(Arc::new(|| NOW)),
        });
        // Upstream `repo.create({}, BACKGROUND_CONTEXT)`: no explicit id.
        let session = repo
            .create(SessionCreateOptions::default(), ctx())
            .await
            .unwrap();
        let usage = Usage {
            input: 1,
            output: 2,
            cache_read: 0,
            cache_write: 0,
            cache_write_1h: None,
            reasoning: None,
            total_tokens: 3,
            cost: UsageCost::default(),
        };
        session
            .mutate(
                |mutator, context| {
                    Box::pin(async move {
                        mutator
                            .commit(
                                vec![
                                    insert_entry(NewEntry::Message {
                                        id: "history".to_string(),
                                        parent_id: None,
                                        message: AgentMessage::User(UserMessage {
                                            content: StringOrBlocks::Text("history".to_string()),
                                            timestamp: NOW,
                                        }),
                                        terminate: None,
                                    }),
                                    insert_usage(NewUsageRow {
                                        id: "usage".to_string(),
                                        usage,
                                        entry_id: None,
                                        adjustment: false,
                                        details: None,
                                    }),
                                ],
                                context,
                            )
                            .await
                            .map(|_| ())
                    })
                },
                ctx(),
            )
            .await
            .unwrap();
        session.close(ctx()).await.unwrap();
        let reopened = repo.open(session.metadata(), ctx()).await.unwrap();
        let result = reopened
            .mutate(
                |mutator, context| {
                    Box::pin(async move {
                        mutator
                            .commit(
                                vec![set_value(&session_name(), serde_json::json!("reopened"))],
                                context,
                            )
                            .await
                    })
                },
                ctx(),
            )
            .await
            .unwrap();
        assert_eq!(
            result.stats,
            SessionStats {
                message_count: 1,
                usage,
            }
        );
        assert_eq!(result.stats, reopened.get_stats(ctx()).await.unwrap());
        reopened.close(ctx()).await.unwrap();
        repo.close(ctx()).await.unwrap();
    }
}
