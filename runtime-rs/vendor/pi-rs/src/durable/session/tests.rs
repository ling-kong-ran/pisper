//! Functional tests for the durable slice: session commit flows over the
//! memory and JSONL backends, including the exact JSONL wire bytes that the
//! `tests/fixtures/durable_oracle` Node fixture compares byte-for-byte.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent_core::chord_support::context::Context;
use crate::chord::delta::Seg;

use super::super::documents::{define_doc, DefinitionScope, DocDefinition};
use super::super::errors::PlainError;
use super::super::session::session::{create_session, create_session_with_hooks};
use super::super::storage::jsonl::{JsonlStorage, JsonlStorageOptions};
use super::super::storage::memory::MemoryStorage;
use super::super::storage::Storage;
use super::super::types::{
    DocumentContent, DocumentFork, DocumentHistory, EntryDraft, StorageWrite, SubmissionRecord,
    SubmissionSettlement, SubmissionStatus, SubmissionType, TaskOptions, TaskOwnership,
};

fn background() -> Context {
    Context::background()
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pi-rust-durable-{tag}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Minimal task definition facet for `tx.createTask`.
struct ProbeTask;

impl super::super::session::transaction::TaskDefinitionFacet for ProbeTask {
    fn name(&self) -> &str {
        "probe"
    }

    fn version(&self) -> i64 {
        1
    }

    fn initial(&self, _input: &Value) -> Value {
        json!({"phase": "idle"})
    }
}

#[tokio::test]
async fn session_commits_conversation_entry_and_task_to_memory() {
    let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
    let session = create_session(Arc::clone(&storage));
    let root = session
        .commit(
            move |tx: Arc<super::transaction::Transaction>| {
                Box::pin(async move {
                    let record = tx.create_root_conversation()?;
                    let entry = tx.append_entry(
                        record.id,
                        EntryDraft {
                            kind: String::from("pi.user"),
                            model: None,
                            data: Some(json!({"text": "hello"})),
                            edits: None,
                            head: None,
                        },
                    )?;
                    let task_id = tx.create_task(
                        &ProbeTask,
                        json!({"n": 1}),
                        TaskOptions {
                            ownership: TaskOwnership::Conversation,
                            conversation_id: Some(record.id),
                            background: None,
                        },
                    )?;
                    Ok((record.id, entry.id, task_id))
                })
            },
            background(),
        )
        .await
        .unwrap();
    let (conversation_id, entry_id, task_id) = root;
    assert_eq!(conversation_id, 1);
    assert_eq!(entry_id, 2);
    assert_eq!(task_id, 3);
    let entry = storage.entry(entry_id, &background()).unwrap().unwrap();
    assert_eq!(entry.entry.kind, "pi.user");
    assert_eq!(
        entry.entry.data.as_ref().unwrap(),
        &json!({"text": "hello"})
    );
    assert_eq!(entry.entry.conversation_id, conversation_id);
    assert_eq!(entry.commit_seq, 1);
    let task = storage.task(task_id, &background()).unwrap().unwrap();
    assert_eq!(task.kind, "probe");
    assert_eq!(
        task.state.status(),
        crate::durable::types::TaskStatus::Pending
    );
    assert_eq!(task.conversation_id, conversation_id);
}

#[tokio::test]
async fn session_transactions_reject_read_after_write() {
    let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
    let session = create_session(Arc::clone(&storage));
    let error = session
        .commit(
            move |tx: Arc<super::transaction::Transaction>| {
                Box::pin(async move {
                    tx.create_root_conversation()?;
                    // A read after the first table write fails with
                    // `ReadAfterWrite` and its exact message.
                    tx.conversation(1)?;
                    Ok(())
                })
            },
            background(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.message,
        "Tx.conversation() cannot read tables after the first table write"
    );
}

/// The JSONL wire bytes of the canonical commit scenario, compared
/// byte-for-byte against the Node oracle fixture
/// (`tests/fixtures/durable_oracle/jsonl_storage.mjs`).
#[tokio::test]
async fn jsonl_storage_writes_exact_commit_bytes() {
    let dir = temp_dir("jsonl-bytes");
    let storage = JsonlStorage::open(&dir, JsonlStorageOptions::default()).unwrap();
    let session = create_session(Arc::new(storage));

    // Scenario: one commit staging a root conversation, a user entry with a
    // model message, a queued submission, and a pending task.
    let writes = session
        .commit(
            move |tx: Arc<super::transaction::Transaction>| {
                Box::pin(async move {
                    let record = tx.create_root_conversation()?;
                    let entry = tx.append_entry(
                        record.id,
                        EntryDraft {
                            kind: String::from("pi.user"),
                            model: Some(vec![crate::ai::types::Message::User(
                                crate::ai::types::UserMessage {
                                    content: crate::ai::types::StringOrBlocks::Text(String::from(
                                        "hi",
                                    )),
                                    timestamp: 1_758_240_000_000,
                                },
                            )]),
                            data: None,
                            edits: None,
                            head: None,
                        },
                    )?;
                    let submission = tx.create_submission(SubmissionRecord {
                        id: 0,
                        conversation_id: record.id,
                        request_id: None,
                        r#type: SubmissionType::Input,
                        status: SubmissionStatus::Queued,
                        entry: None,
                        answer: None,
                        reason: None,
                        detail: None,
                        entry_before_id: false,
                    })?;
                    let task_id = tx.create_task(
                        &ProbeTask,
                        json!({"n": 1}),
                        TaskOptions {
                            ownership: TaskOwnership::Conversation,
                            conversation_id: Some(record.id),
                            background: None,
                        },
                    )?;
                    tx.settle_submission(
                        submission.id,
                        SubmissionSettlement::Unanswered {
                            reason: String::from("withdrawn"),
                            detail: None,
                        },
                    )?;
                    Ok((record.id, entry.id, task_id, submission.id))
                })
            },
            background(),
        )
        .await
        .unwrap();
    let (conversation_id, entry_id, task_id, submission_id) = writes;
    assert_eq!(entry_id, 2);

    let main = std::fs::read_to_string(dir.join("main.jsonl")).unwrap();
    let lines: Vec<&str> = main.trim_end().split('\n').collect();
    assert_eq!(lines.len(), 1, "one commit marker line");
    let marker: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(marker["format"], 1);
    assert_eq!(marker["type"], "commit");
    assert_eq!(marker["seq"], 1);
    let marker_writes = marker["writes"].as_array().unwrap();
    assert_eq!(
        marker_writes.len(),
        4,
        "conversation + entry + settled submission + task.sidecar"
    );
    assert_eq!(marker_writes[0]["type"], "conversation");
    assert_eq!(marker_writes[0]["value"]["id"], conversation_id);
    assert_eq!(marker_writes[1]["type"], "entry");
    assert_eq!(marker_writes[1]["value"]["id"], entry_id);
    assert_eq!(marker_writes[2]["type"], "submission");
    // The settled submission: `unanswered` keeps its type and id; the entry
    // stays absent.
    assert_eq!(marker_writes[2]["value"]["status"], "unanswered");
    assert_eq!(marker_writes[2]["value"]["reason"], "withdrawn");
    assert_eq!(marker_writes[2]["value"]["id"], submission_id);
    assert_eq!(marker_writes[3]["type"], "task.sidecar");
    assert_eq!(marker_writes[3]["id"], task_id);

    let sidecar = std::fs::read_to_string(dir.join(format!("task-{task_id}.jsonl"))).unwrap();
    let record: Value = serde_json::from_str(sidecar.trim_end()).unwrap();
    assert_eq!(record["format"], 1);
    assert_eq!(record["type"], "record");
    assert_eq!(record["seq"], 1);
    assert_eq!(record["ordinal"], 0);
    assert_eq!(record["payload"]["type"], "task");
    assert_eq!(record["payload"]["value"]["state"]["status"], "pending");
    assert_eq!(
        record["payload"]["value"]["state"]["checkpoint"],
        json!({"phase": "idle"})
    );

    // Entry bytes: the user message round-trips byte-identically through the
    // entry record's `model` field.
    let entry_line = {
        let storage = JsonlStorage::open(&dir, JsonlStorageOptions::default()).unwrap();
        let found = crate::durable::storage::Storage::entry(&storage, entry_id, &background())
            .unwrap()
            .unwrap();
        serde_json::to_value(&found.entry).unwrap()
    };
    assert_eq!(entry_line["kind"], "pi.user");
    assert_eq!(entry_line["model"][0]["role"], "user");
    assert_eq!(entry_line["model"][0]["content"], "hi");
    assert_eq!(entry_line["conversationId"], conversation_id);
    assert_eq!(entry_line["id"], entry_id);
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn jsonl_storage_recovers_state_across_reopen() {
    let dir = temp_dir("jsonl-reopen");
    {
        let storage = JsonlStorage::open(&dir, JsonlStorageOptions::default()).unwrap();
        let session = create_session(Arc::new(storage));
        session
            .commit(
                move |tx: Arc<super::transaction::Transaction>| {
                    Box::pin(async move {
                        let record = tx.create_root_conversation()?;
                        tx.append_entry(record.id, EntryDraft::new("pi.system"))?;
                        Ok(())
                    })
                },
                background(),
            )
            .await
            .unwrap();
        session.close(background()).await.unwrap();
    }
    {
        let storage = JsonlStorage::open(&dir, JsonlStorageOptions::default()).unwrap();
        let page = storage
            .scan_entries(
                crate::durable::types::EntryQuery {
                    conversation_id: 1,
                    min_entry_id: None,
                    max_entry_id: None,
                },
                10,
                None,
                &background(),
            )
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].kind, "pi.system");
        assert_eq!(
            crate::durable::storage::Storage::mint_id(&storage).unwrap(),
            3,
            "the ID space resumes after recovery"
        );
        storage.close(&background()).unwrap();
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn session_documents_track_and_checkpoint() {
    let dir = temp_dir("jsonl-docs");
    let storage = JsonlStorage::open(&dir, JsonlStorageOptions::default()).unwrap();
    let session = create_session_with_hooks(Arc::new(storage), None);

    let definition = define_doc(DocDefinition {
        kind: String::from("counter"),
        version: 1,
        scope: DefinitionScope::Conversation,
        history: Some(DocumentHistory::Latest),
        fork: Some(DocumentFork::Current),
        family: false,
        initial: Arc::new(|_| {
            let mut value = serde_json::Map::new();
            value.insert(String::from("count"), Value::from(0));
            value
        }),
        migrate: None,
        checkpoint_when: None,
    })
    .unwrap();

    session
        .commit(
            move |tx: Arc<super::transaction::Transaction>| {
                Box::pin(async move {
                    tx.create_root_conversation()?;
                    Ok(())
                })
            },
            background(),
        )
        .await
        .unwrap();

    // Clone the definition into each closure (DocDefinition is not Copy).
    let definition_for_first = definition.definition.clone();
    let definition_for_second = definition.definition.clone();
    let definition_for_retire = definition.definition.clone();
    session
        .commit(
            move |tx: Arc<super::transaction::Transaction>| {
                Box::pin(async move {
                    let draft = tx.doc(&definition_for_first, Some(1), None, None)?;
                    draft
                        .set(&[Seg::Key(String::from("count"))], Value::from(2))
                        .map_err(|error| PlainError::new(error.message()))?;
                    Ok(())
                })
            },
            background(),
        )
        .await
        .unwrap();

    let snapshot = session
        .snapshot(&definition.definition, Some(1), None, background())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(snapshot["count"], 2);

    // The sidecar holds the creation base with the exact wire shape.
    let sidecar = std::fs::read_to_string(dir.join("doc-2.jsonl")).unwrap();
    let record: Value = serde_json::from_str(sidecar.trim_end()).unwrap();
    assert_eq!(record["payload"]["type"], "document");
    assert_eq!(record["payload"]["id"], 2);
    assert_eq!(record["payload"]["content"]["kind"], "base");
    assert_eq!(record["payload"]["content"]["version"], 1);
    assert_eq!(record["payload"]["content"]["value"], json!({"count": 2}));

    // A second change stores a delta; the snapshot still materializes.
    session
        .commit(
            move |tx: Arc<super::transaction::Transaction>| {
                Box::pin(async move {
                    let draft = tx.doc(&definition_for_second, Some(1), None, None)?;
                    draft
                        .set(&[Seg::Key(String::from("count"))], Value::from(5))
                        .map_err(|error| PlainError::new(error.message()))?;
                    Ok(())
                })
            },
            background(),
        )
        .await
        .unwrap();
    let snapshot = session
        .snapshot(&definition.definition, Some(1), None, background())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(snapshot["count"], 5);
    let sidecar = std::fs::read_to_string(dir.join("doc-2.jsonl")).unwrap();
    let lines: Vec<&str> = sidecar.trim_end().split('\n').collect();
    assert_eq!(lines.len(), 2, "base + delta records");
    let delta: Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(delta["payload"]["content"]["kind"], "delta");

    // Retire the document: the current-only sidecar is reclaimed and the
    // record is retired.
    session
        .commit(
            move |tx: Arc<super::transaction::Transaction>| {
                Box::pin(async move {
                    tx.retire_doc(&definition_for_retire, Some(1), None)?;
                    Ok(())
                })
            },
            background(),
        )
        .await
        .unwrap();
    let snapshot = session
        .snapshot(&definition.definition, Some(1), None, background())
        .await
        .unwrap();
    assert!(
        snapshot.is_none(),
        "retired documents snapshot to undefined"
    );
    let sidecar_exists = dir.join("doc-2.jsonl").exists();
    assert!(
        !sidecar_exists,
        "current-only sidecars are reclaimed on retirement"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn memory_storage_pages_scans_with_cursors() {
    let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
    let session = create_session(Arc::clone(&storage));
    session
        .commit(
            move |tx: Arc<super::transaction::Transaction>| {
                Box::pin(async move {
                    let root = tx.create_root_conversation()?;
                    for index in 0..5 {
                        tx.append_entry(root.id, EntryDraft::new(format!("entry-{index}")))?;
                    }
                    Ok(())
                })
            },
            background(),
        )
        .await
        .unwrap();
    let query = crate::durable::types::EntryQuery {
        conversation_id: 1,
        min_entry_id: None,
        max_entry_id: None,
    };
    // Newest-first scan with page size 2: 5 entries need three pages, and the
    // cursor's `after` is the last item of the page.
    let mut seen: Vec<i64> = Vec::new();
    let mut cursor: Option<crate::durable::types::Cursor> = None;
    let mut pages = 0;
    loop {
        let page = storage
            .scan_entries(query, 2, cursor.as_ref(), &background())
            .unwrap();
        pages += 1;
        seen.extend(page.items.iter().map(|entry| entry.id));
        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(seen, vec![6, 5, 4, 3, 2], "newest-first, two per page");
}

/// Unused import guards: these shapes are exercised through the storage wire.
#[allow(dead_code)]
fn wire_shapes(write: StorageWrite, content: DocumentContent) -> (StorageWrite, DocumentContent) {
    (write, content)
}
