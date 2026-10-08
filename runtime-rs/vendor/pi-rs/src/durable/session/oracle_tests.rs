//! Byte-oracle tests against `tests/fixtures/durable_oracle/durable_oracle.json`
//! (captured by `capture_durable_oracle.mjs` from the read-only upstream
//! sources; see the fixture header for the determinism contract). Every
//! scenario mirrors the capture one-to-one; the JSONL line strings and record
//! shapes must match byte-for-byte.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use crate::agent_core::chord_support::context::Context;
use crate::chord::delta::Seg;

use super::super::documents::{define_doc, DefinitionScope, DocDefinition};
use super::super::session::session::create_session;
use super::super::session::transaction::Transaction;
use super::super::storage::jsonl::{JsonlStorage, JsonlStorageOptions};
use super::super::storage::Storage;
use super::super::types::{
    EntryDraft, SubmissionRecord, SubmissionSettlement, SubmissionType, TaskOptions, TaskOwnership,
};

fn oracle() -> Value {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = manifest_dir.join("tests/fixtures/durable_oracle/durable_oracle.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pi-rust-durable-oracle-{tag}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct ProbeTask;

impl super::super::session::transaction::TaskDefinitionFacet for ProbeTask {
    fn name(&self) -> &str {
        "probe"
    }

    fn version(&self) -> i64 {
        1
    }

    fn initial(&self, _input: &serde_json::Value) -> serde_json::Value {
        serde_json::json!({"phase": "idle"})
    }
}

/// Scenario 1 (`commit_bytes`): reserved root conversation, `pi.user` entry
/// with a user message, queued input submission settled `unanswered`, pending
/// task. The main.jsonl and task-4.jsonl line bytes must equal the Node
/// capture byte-for-byte.
#[tokio::test]
async fn oracle_commit_bytes_match() {
    let expected = oracle()["commit_bytes"].clone();
    let dir = temp_dir("commit-bytes");
    let storage = Arc::new(JsonlStorage::open(&dir, JsonlStorageOptions::default()).unwrap());
    let session = create_session(Arc::clone(&storage) as Arc<dyn Storage>);

    let ids = session
        .commit(
            move |tx: Arc<Transaction>| {
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
                    let submission = tx.create_submission(SubmissionRecord::queued(
                        record.id,
                        None,
                        SubmissionType::Input,
                        0,
                    ))?;
                    let task_id = tx.create_task(
                        &ProbeTask,
                        serde_json::json!({"n": 1}),
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
                    Ok((record.id, entry.id, submission.id, task_id))
                })
            },
            Context::background(),
        )
        .await
        .unwrap();
    let (conversation_id, entry_id, submission_id, task_id) = ids;

    // Allocation order must match the capture exactly.
    assert_eq!(
        serde_json::json!({
            "conversation": conversation_id,
            "entry": entry_id,
            "submission": submission_id,
            "task": task_id,
        }),
        expected["ids"],
    );

    let main = std::fs::read_to_string(dir.join("main.jsonl")).unwrap();
    let mut main_lines: Vec<&str> = main.split('\n').collect();
    main_lines.pop();
    let expected_main: Vec<String> = expected["mainLines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line.as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        main_lines, expected_main,
        "main.jsonl lines must match the Node capture byte-for-byte"
    );

    let sidecar_name = expected["sidecarFile"].as_str().unwrap();
    let sidecar = std::fs::read_to_string(dir.join(sidecar_name)).unwrap();
    let mut sidecar_lines: Vec<&str> = sidecar.split('\n').collect();
    sidecar_lines.pop();
    let expected_sidecar: Vec<String> = expected["sidecarLines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line.as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        sidecar_lines, expected_sidecar,
        "task sidecar lines must match the Node capture byte-for-byte"
    );

    // The round-tripped entry record: the model message byte-identical.
    let found = storage
        .entry(entry_id, &Context::background())
        .unwrap()
        .unwrap();
    let entry_value = serde_json::to_value(&found.entry).unwrap();
    assert_eq!(entry_value, expected["entryRecord"]);
    assert_eq!(
        found.commit_seq,
        expected["entryCommitSeq"].as_i64().unwrap()
    );
    storage.close(&Context::background()).unwrap();
    std::fs::remove_dir_all(&dir).ok();
}

/// Scenario 2 (`document_bytes`): conversation document create (base), change
/// (delta), retire (sidecar reclaimed). The doc-2.jsonl line bytes and the
/// main.jsonl marker lines must equal the Node capture byte-for-byte.
#[tokio::test]
async fn oracle_document_bytes_match() {
    let expected = oracle()["document_bytes"].clone();
    let dir = temp_dir("doc-bytes");
    let storage = Arc::new(JsonlStorage::open(&dir, JsonlStorageOptions::default()).unwrap());
    let session = create_session(Arc::clone(&storage) as Arc<dyn Storage>);

    let definition = define_doc(DocDefinition {
        kind: String::from("counter"),
        version: 1,
        scope: DefinitionScope::Conversation,
        history: Some(crate::durable::types::DocumentHistory::Latest),
        fork: Some(crate::durable::types::DocumentFork::Current),
        family: false,
        initial: Arc::new(|_| {
            let mut value = serde_json::Map::new();
            value.insert(String::from("count"), serde_json::Value::from(0));
            value
        }),
        migrate: None,
        checkpoint_when: None,
    })
    .unwrap();
    let definition = &definition.definition;

    session
        .commit(
            move |tx: Arc<Transaction>| {
                Box::pin(async move {
                    tx.create_root_conversation()?;
                    Ok(())
                })
            },
            Context::background(),
        )
        .await
        .unwrap();

    let definition_for_create = definition.clone();
    session
        .commit(
            move |tx: Arc<Transaction>| {
                Box::pin(async move {
                    let draft = tx.doc(&definition_for_create, Some(1), None, None)?;
                    draft
                        .set(
                            &[Seg::Key(String::from("count"))],
                            serde_json::Value::from(2),
                        )
                        .map_err(|error| {
                            crate::durable::errors::PlainError::new(error.message())
                        })?;
                    Ok(())
                })
            },
            Context::background(),
        )
        .await
        .unwrap();

    let snapshot = session
        .snapshot(definition, Some(1), None, Context::background())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&snapshot).unwrap(),
        expected["snapshotAfterCreate"]
    );
    let doc_text = std::fs::read_to_string(dir.join("doc-2.jsonl")).unwrap();
    let mut doc_lines: Vec<&str> = doc_text.split('\n').collect();
    doc_lines.pop();
    let expected_lines: Vec<String> = expected["docLinesAfterCreate"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line.as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        doc_lines, expected_lines,
        "document sidecar bytes after create"
    );

    let definition_for_change = definition.clone();
    session
        .commit(
            move |tx: Arc<Transaction>| {
                Box::pin(async move {
                    let draft = tx.doc(&definition_for_change, Some(1), None, None)?;
                    draft
                        .set(
                            &[Seg::Key(String::from("count"))],
                            serde_json::Value::from(5),
                        )
                        .map_err(|error| {
                            crate::durable::errors::PlainError::new(error.message())
                        })?;
                    Ok(())
                })
            },
            Context::background(),
        )
        .await
        .unwrap();
    let snapshot = session
        .snapshot(definition, Some(1), None, Context::background())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&snapshot).unwrap(),
        expected["snapshotAfterChange"]
    );
    let doc_text = std::fs::read_to_string(dir.join("doc-2.jsonl")).unwrap();
    let mut doc_lines: Vec<&str> = doc_text.split('\n').collect();
    doc_lines.pop();
    let expected_lines: Vec<String> = expected["docLinesAfterChange"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line.as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        doc_lines, expected_lines,
        "document sidecar bytes after change"
    );

    let definition_for_retire = definition.clone();
    session
        .commit(
            move |tx: Arc<Transaction>| {
                Box::pin(async move {
                    tx.retire_doc(&definition_for_retire, Some(1), None)?;
                    Ok(())
                })
            },
            Context::background(),
        )
        .await
        .unwrap();
    let snapshot = session
        .snapshot(definition, Some(1), None, Context::background())
        .await
        .unwrap();
    assert_eq!(
        snapshot.map(|value| serde_json::to_value(value).unwrap()),
        if expected["snapshotAfterRetire"].is_null() {
            None
        } else {
            Some(expected["snapshotAfterRetire"].clone())
        }
    );
    assert_eq!(
        dir.join("doc-2.jsonl").exists(),
        expected["sidecarExistsAfterRetire"].as_bool().unwrap(),
        "current-only sidecars are reclaimed on retirement"
    );
    let main = std::fs::read_to_string(dir.join("main.jsonl")).unwrap();
    let mut main_lines: Vec<&str> = main.split('\n').collect();
    main_lines.pop();
    let expected_main: Vec<String> = expected["mainLines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line.as_str().unwrap().to_string())
        .collect();
    assert_eq!(main_lines, expected_main, "main.jsonl marker lines");
    storage.close(&Context::background()).unwrap();
    std::fs::remove_dir_all(&dir).ok();
}

/// Scenario 3 (`read_after_write`): the exact `ReadAfterWrite` message text.
#[tokio::test]
async fn oracle_read_after_write_message() {
    let expected = oracle()["read_after_write"].clone();
    let storage: Arc<dyn Storage> = Arc::new(super::super::storage::memory::MemoryStorage::new());
    let session = create_session(Arc::clone(&storage));
    let error = session
        .commit(
            move |tx: Arc<Transaction>| {
                Box::pin(async move {
                    tx.create_root_conversation()?;
                    tx.conversation(1)?;
                    Ok(())
                })
            },
            Context::background(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.message,
        expected["error"]["message"].as_str().unwrap()
    );
}
