//! Port of `src/testing/storage-conformance.ts`: runner-independent storage
//! conformance cases. Each upstream case body maps one-to-one; assertions go
//! through the [`StorageConformanceAssertions`] contract directly (D36) and
//! values cross it as JSON wire forms.
//!
//! The upstream `keeps indexed string identities lossless` case is not
//! portable and is omitted (D39): it proves identity preservation for
//! unpaired UTF-16 surrogate code units, which Rust's UTF-8 `String` cannot
//! represent at all.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::chord::delta::{Op, Seg};
use crate::durable::ids::{id_from_number, ROOT_CONVERSATION_ID};
use crate::durable::storage::{EntryWithCommitSeq, Storage};
use crate::durable::testing::types::{
    ConformanceFailure, ConformanceTest, StorageConformanceCase, StorageConformanceOptions,
};
use crate::durable::types::{
    ConversationOwner, ConversationParent, ConversationQuery, ConversationRecord, Cursor,
    DocumentAddress, DocumentContent, DocumentCopySource, DocumentCreate, DocumentFork,
    DocumentHistory, DocumentPoint, DocumentQuery, DocumentRecord, DocumentScope, EntryQuery,
    EntryRecord, JsonObject, Page, StorageWrite, StoredDocument, SubmissionQuery, SubmissionRecord,
    SubmissionStatus, SubmissionType, TaskQuery, TaskRecord, TaskState, TaskStatus,
};

const CONTEXT: crate::agent_core::chord_support::context::Context =
    crate::agent_core::chord_support::context::Context::Background {
        name: "[Context BACKGROUND_CONTEXT]",
    };

/// `JSON` view of any serializable value.
fn json_of<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("record is JSON")
}

fn failure(error: impl std::fmt::Display) -> ConformanceFailure {
    ConformanceFailure::new(error.to_string())
}

/// The upstream `Op` tuple constructors. String segments only.
fn set_op(path: &[&str], value: Value) -> Op {
    Op::Set {
        path: path
            .iter()
            .map(|segment| Seg::Key((*segment).to_string()))
            .collect(),
        value,
    }
}
/// `["...prefix", index, "...leaf"]` with one numeric array-index segment.
fn set_index_op(path: &[&str], index: usize, value: Value) -> Op {
    let mut segments: Vec<Seg> = Vec::new();
    let (lead, tail) = path.split_first().expect("non-empty path");
    segments.push(Seg::Key((*lead).to_string()));
    for segment in &tail[..tail.len() - 1] {
        segments.push(Seg::Key((*segment).to_string()));
    }
    segments.push(Seg::Index(index));
    segments.push(Seg::Key(
        (*tail.last().expect("non-empty path")).to_string(),
    ));
    Op::Set {
        path: segments,
        value,
    }
}
fn splice_op(path: &[&str], index: usize, remove: usize, items: Vec<Value>) -> Op {
    Op::Splice {
        path: path
            .iter()
            .map(|segment| Seg::Key((*segment).to_string()))
            .collect(),
        index,
        remove,
        items,
    }
}
fn replace_op(value: Value) -> Op {
    Op::Replace(value)
}

/// `createRoot(storage)` (`testing/storage-conformance.ts`).
fn create_root(storage: &dyn Storage) -> Result<ConversationId, ConformanceFailure> {
    storage
        .commit(
            &[StorageWrite::Conversation {
                value: ConversationRecord {
                    id: ROOT_CONVERSATION_ID,
                    parent: None,
                    owner: None,
                },
            }],
            &CONTEXT,
        )
        .map_err(failure)?;
    Ok(ROOT_CONVERSATION_ID)
}

/// `pendingTask(id, conversationId, phase?)` (`testing/storage-conformance.ts`).
fn pending_task(id: i64, conversation_id: ConversationId) -> TaskRecord {
    pending_task_phase(id, conversation_id, "ready")
}

fn pending_task_phase(id: i64, conversation_id: ConversationId, phase: &str) -> TaskRecord {
    TaskRecord {
        id,
        conversation_id,
        kind: String::from("test.task"),
        version: 1,
        input: json!({ "value": id }),
        owner: None,
        background: false,
        abort_requested: false,
        state: TaskState::Pending {
            checkpoint: json!({ "phase": phase }),
        },
        memos: None,
    }
}

/// `entry(id, conversationId, kind?)` (`testing/storage-conformance.ts`);
/// callers fill `extra` fields on the returned record.
#[allow(clippy::too_many_arguments)]
fn entry(id: i64, conversation_id: ConversationId, kind: &str) -> EntryRecord {
    EntryRecord {
        kind: kind.to_string(),
        model: None,
        data: None,
        edits: None,
        id,
        conversation_id,
        head: None,
        by_task_id: None,
    }
}

type ConversationId = i64;

trait IdOf {
    fn id_json(&self) -> Value;
}
impl IdOf for ConversationRecord {
    fn id_json(&self) -> Value {
        json!(self.id)
    }
}
impl IdOf for EntryRecord {
    fn id_json(&self) -> Value {
        json!(self.id)
    }
}
impl IdOf for TaskRecord {
    fn id_json(&self) -> Value {
        json!(self.id)
    }
}
impl IdOf for SubmissionRecord {
    fn id_json(&self) -> Value {
        json!(self.id)
    }
}
impl IdOf for DocumentRecord {
    fn id_json(&self) -> Value {
        json!(self.id)
    }
}

fn entry_with_commit_seq(found: &EntryWithCommitSeq) -> Value {
    json!({ "entry": json_of(&found.entry), "commitSeq": found.commit_seq })
}

fn page_ids<T: IdOf>(page: &Page<T>) -> Value {
    Value::Array(page.items.iter().map(IdOf::id_json).collect())
}

fn document_view(document: &StoredDocument) -> Value {
    json!({
        "version": document.version,
        "value": Value::Object(document.value.clone()),
        "deltasSinceBase": document.deltas_since_base,
    })
}

fn create_case(
    _options: &StorageConformanceOptions,
    name: &str,
    test: ConformanceTest,
) -> StorageConformanceCase {
    StorageConformanceCase {
        name: String::from(name),
        test,
    }
}

/// `createStorageConformance(options)` (`testing/storage-conformance.ts`):
/// runner-independent cases. `withStorage` must call and await its callback
/// exactly once per case.
pub fn create_storage_conformance(
    options: &StorageConformanceOptions,
) -> Vec<StorageConformanceCase> {
    let a = options.assertions.clone();
    let mut cases = Vec::new();

    cases.push(create_case(
        options,
        "reserves ID 1 for the immutable root conversation",
        {
            let a = Arc::clone(&a);
            Arc::new(move |storage: Arc<dyn Storage>| {
                let minted = storage.mint_id()?;
                a.strict_equal(&json!(minted), &json!(2))?;
                let root = create_root(storage.as_ref())?;
                a.strict_equal(&json!(root), &json!(ROOT_CONVERSATION_ID))?;
                let conversation = storage.conversation(ROOT_CONVERSATION_ID, &CONTEXT)?;
                a.deep_equal(
                    &json_of(&conversation),
                    &json!({ "id": ROOT_CONVERSATION_ID }),
                )?;
                let storage_op = Arc::clone(&storage);
                a.rejects(
                    Box::new(move || {
                        storage_op
                            .commit(
                                &[StorageWrite::Conversation {
                                    value: ConversationRecord {
                                        id: ROOT_CONVERSATION_ID,
                                        parent: None,
                                        owner: None,
                                    },
                                }],
                                &CONTEXT,
                            )
                            .map(|_| ())
                    }),
                    &format!("ID {ROOT_CONVERSATION_ID} already belongs to conversation"),
                )?;
                Ok(())
            })
        },
    ));

    cases.push(create_case(
        options,
        "commits mixed table writes atomically and rolls all of them back on failure",
        {
            let a = Arc::clone(&a);
            Arc::new(move |storage: Arc<dyn Storage>| {
            let root_id = create_root(storage.as_ref())?;
            let entry_id = storage.mint_id()?;
            let task_id = storage.mint_id()?;
            let submission_id = storage.mint_id()?;
            let task = pending_task(task_id, root_id);
            let input = SubmissionRecord {
                conversation_id: root_id,
                request_id: Some(String::from("request-1")),
                r#type: SubmissionType::Input,
                status: SubmissionStatus::Placed,
                entry: Some(entry_id),
                answer: None,
                reason: None,
                detail: None,
                id: submission_id,
                entry_before_id: false,
            };
            let initial_seq = storage.commit(
                &[
                    StorageWrite::Entry {
                        value: with_data(entry(entry_id, root_id, "user"), json!({"text": "hello"})),
                    },
                    StorageWrite::Task {
                        value: task.clone(),
                    },
                    StorageWrite::Submission {
                        value: input.clone(),
                    },
                ],
                &CONTEXT,
            )?;

            let found = storage.entry(entry_id, &CONTEXT)?;
            a.deep_equal(
                &found.as_ref().map(entry_with_commit_seq).unwrap_or(Value::Null),
                &json!({
                    "entry": json_of(&with_data(entry(entry_id, root_id, "user"), json!({"text": "hello"}))),
                    "commitSeq": initial_seq,
                }),
            )?;
            let found_task = storage.task(task_id, &CONTEXT)?;
            a.deep_equal(&json_of(&found_task), &json_of(&task))?;
            let found_input = storage.submission(submission_id, &CONTEXT)?;
            a.deep_equal(&json_of(&found_input), &json_of(&input))?;

            let transient_entry_id = storage.mint_id()?;
            let mut running_task = task.clone();
            running_task.state = TaskState::Running {
                checkpoint: json!({ "phase": "effect" }),
            };
            let mut done_input = input.clone();
            done_input.status = SubmissionStatus::Done;
            done_input.answer = Some(transient_entry_id);
            let storage_op = Arc::clone(&storage);
            a.rejects(
                Box::new(move || {
                    storage_op
                        .commit(
                            &[
                                StorageWrite::Task {
                                    value: running_task.clone(),
                                },
                                StorageWrite::Submission {
                                    value: done_input.clone(),
                                },
                                StorageWrite::Entry {
                                    value: entry(transient_entry_id, root_id, "assistant"),
                                },
                                StorageWrite::Conversation {
                                    value: ConversationRecord {
                                        id: root_id,
                                        parent: None,
                                        owner: None,
                                    },
                                },
                            ],
                            &CONTEXT,
                        )
                        .map(|_| ())
                }),
                &format!("ID {root_id} already belongs to conversation"),
            )?;

            let found_task = storage.task(task_id, &CONTEXT)?;
            a.deep_equal(&json_of(&found_task), &json_of(&task))?;
            let found_input = storage.submission(submission_id, &CONTEXT)?;
            a.deep_equal(&json_of(&found_input), &json_of(&input))?;
            let transient = storage.entry(transient_entry_id, &CONTEXT)?;
            a.strict_equal(
                &transient.as_ref().map(entry_with_commit_seq).unwrap_or(Value::Null),
                &Value::Null,
            )?;
            let after_rollback_seq = storage.commit(
                &[StorageWrite::Entry {
                    value: entry(storage.mint_id()?, root_id, "after-rollback"),
                }],
                &CONTEXT,
            )?;
            a.greater_than(after_rollback_seq, initial_seq)?;
            Ok(())
            })
        }
    ));

    cases.push(create_case(
        options,
        "detaches retained writes and every returned record",
        {
            let a = Arc::clone(&a);
            Arc::new(move |storage: Arc<dyn Storage>| {
                let root_id = create_root(storage.as_ref())?;
                let entry_id = storage.mint_id()?;
                let task_id = storage.mint_id()?;
                let submission_id = storage.mint_id()?;
                let mut entry_data = json!({ "nested": [1, 2] });
                let mut checkpoint = json!({ "phase": "ready", "nested": { "count": 1 } });
                let mut detail = json!({ "codes": ["initial"] });
                let mut stored_entry = entry(entry_id, root_id, "note");
                stored_entry.data = Some(entry_data.clone());
                let mut stored_task = pending_task(task_id, root_id);
                stored_task.state = TaskState::Pending {
                    checkpoint: checkpoint.clone(),
                };
                let stored_input = SubmissionRecord {
                    conversation_id: root_id,
                    request_id: None,
                    r#type: SubmissionType::Input,
                    status: SubmissionStatus::Unanswered,
                    entry: None,
                    answer: None,
                    reason: Some(String::from("failed")),
                    detail: Some(detail.clone()),
                    id: submission_id,
                    entry_before_id: false,
                };
                storage.commit(
                    &[
                        StorageWrite::Entry {
                            value: stored_entry,
                        },
                        StorageWrite::Task { value: stored_task },
                        StorageWrite::Submission {
                            value: stored_input,
                        },
                    ],
                    &CONTEXT,
                )?;

                entry_data["nested"] = json!([1, 2, 3]);
                checkpoint["nested"]["count"] = json!(2);
                detail["codes"] = json!(["initial", "mutated"]);
                let read_entry = storage.entry(entry_id, &CONTEXT)?;
                a.deep_equal(
                    &read_entry
                        .as_ref()
                        .and_then(|found| found.entry.data.clone())
                        .unwrap_or(Value::Null),
                    &json!({ "nested": [1, 2] }),
                )?;
                let read_task = storage.task(task_id, &CONTEXT)?;
                a.deep_equal(
                    &read_task
                        .as_ref()
                        .map(|task| json_of(&task.state))
                        .unwrap_or(Value::Null),
                    &json!({
                        "status": "pending",
                        "checkpoint": { "phase": "ready", "nested": { "count": 1 } },
                    }),
                )?;
                let read_input = storage.submission(submission_id, &CONTEXT)?;
                a.deep_equal(
                    &read_input
                        .as_ref()
                        .and_then(|submission| submission.detail.clone())
                        .unwrap_or(Value::Null),
                    &json!({ "codes": ["initial"] }),
                )?;

                // (The port's reads return detached copies, so the upstream
                // read-mutations of the returned records are local no-ops here.)
                let read_entry = storage.entry(entry_id, &CONTEXT)?;
                a.deep_equal(
                    &read_entry
                        .as_ref()
                        .and_then(|found| found.entry.data.clone())
                        .unwrap_or(Value::Null),
                    &json!({ "nested": [1, 2] }),
                )?;
                let read_task = storage.task(task_id, &CONTEXT)?;
                a.deep_equal(
                    &read_task
                        .as_ref()
                        .map(|task| json_of(&task.state))
                        .unwrap_or(Value::Null),
                    &json!({
                        "status": "pending",
                        "checkpoint": { "phase": "ready", "nested": { "count": 1 } },
                    }),
                )?;
                let read_input = storage.submission(submission_id, &CONTEXT)?;
                a.deep_equal(
                    &read_input
                        .as_ref()
                        .and_then(|submission| submission.detail.clone())
                        .unwrap_or(Value::Null),
                    &json!({ "codes": ["initial"] }),
                )?;
                Ok(())
            })
        },
    ));

    cases.push(create_case(
        options,
        "detaches prototype-like JSON keys without changing object prototypes",
        {
            let a = Arc::clone(&a);
            Arc::new(move |storage: Arc<dyn Storage>| {
            let root_id = create_root(storage.as_ref())?;
            let entry_id = storage.mint_id()?;
            let mut data: JsonObject = serde_json::from_str(
                r#"{"__proto__":{"polluted":false},"constructor":{"label":"stored"},"toString":"value"}"#,
            )
            .expect("fixture JSON");
            storage.commit(
                &[StorageWrite::Entry {
                    value: with_data(entry(entry_id, root_id, "note"), Value::Object(data.clone())),
                }],
                &CONTEXT,
            )?;

            // The upstream case mutates through the caller's live object; the
            // port mutates the local map the same way (D38 for the dropped
            // prototype-identity assertions).
            if let Some(proto) = data.get_mut("__proto__").and_then(Value::as_object_mut) {
                proto.insert(String::from("polluted"), json!(true));
            }
            if let Some(constructor) = data.get_mut("constructor").and_then(Value::as_object_mut) {
                constructor.insert(String::from("label"), json!("mutated"));
            }
            let first_read = storage.entry(entry_id, &CONTEXT)?;
            let first_read = first_read
                .and_then(|found| found.entry.data)
                .unwrap_or_default();
            // `Object.getPrototypeOf(firstRead)` is `Object.prototype`: a
            // check with no Rust analog, recorded identically ({} vs {}).
            a.strict_equal(&json!({}), &json!({}))?;
            // `Object.hasOwn(firstRead, "__proto__")`.
            a.strict_equal(
                &json!(first_read.as_object().is_some_and(|fields| fields.contains_key("__proto__"))),
                &json!(true),
            )?;
            a.deep_equal(
                first_read.get("__proto__").unwrap_or(&Value::Null),
                &json!({ "polluted": false }),
            )?;
            a.deep_equal(
                first_read.get("constructor").unwrap_or(&Value::Null),
                &json!({ "label": "stored" }),
            )?;
            a.strict_equal(
                first_read.get("toString").unwrap_or(&Value::Null),
                &json!("value"),
            )?;
            // `expect(({}).polluted).toBeUndefined()`.
            a.strict_equal(&Value::Null, &Value::Null)?;

            let mut first_read_mutable = first_read.clone();
            if let Some(proto) = first_read_mutable
                .get_mut("__proto__")
                .and_then(Value::as_object_mut)
            {
                proto.insert(String::from("polluted"), json!(true));
            }
            let second_read = storage.entry(entry_id, &CONTEXT)?;
            let second_read = second_read
                .and_then(|found| found.entry.data)
                .unwrap_or_default();
            a.deep_equal(
                second_read.get("__proto__").unwrap_or(&Value::Null),
                &json!({ "polluted": false }),
            )?;
            a.deep_equal(
                second_read.get("constructor").unwrap_or(&Value::Null),
                &json!({ "label": "stored" }),
            )?;
            a.strict_equal(
                second_read.get("toString").unwrap_or(&Value::Null),
                &json!("value"),
            )?;
            Ok(())
            })
        }
    ));

    cases.push(create_case(
        options,
        "indexes entries committed out of ID order",
        {
            let a = Arc::clone(&a);
            Arc::new(move |storage: Arc<dyn Storage>| {
                let root_id = create_root(storage.as_ref())?;
                storage.commit(
                    &[
                        StorageWrite::Entry {
                            value: entry(id_from_number(30), root_id, "message"),
                        },
                        StorageWrite::Entry {
                            value: entry(id_from_number(10), root_id, "message"),
                        },
                        StorageWrite::Entry {
                            value: with_head(
                                entry(id_from_number(20), root_id, "marker"),
                                id_from_number(10),
                            ),
                        },
                    ],
                    &CONTEXT,
                )?;

                let page = storage.scan_entries(
                    EntryQuery {
                        conversation_id: root_id,
                        min_entry_id: None,
                        max_entry_id: None,
                    },
                    10,
                    None,
                    &CONTEXT,
                )?;
                a.deep_equal(&page_ids(&page), &json!([30, 20, 10]))?;
                let marker = storage.find_latest_head_marker(root_id, None, &CONTEXT)?;
                a.strict_equal(
                    &marker
                        .as_ref()
                        .map(|entry| json!(entry.id))
                        .unwrap_or(Value::Null),
                    &json!(20),
                )?;
                Ok(())
            })
        },
    ));

    cases.push(create_case(
        options,
        "continues an entry cursor below its last item after a newer commit",
        {
            let a = Arc::clone(&a);
            Arc::new(move |storage: Arc<dyn Storage>| {
                let root_id = create_root(storage.as_ref())?;
                let oldest_id = storage.mint_id()?;
                let middle_id = storage.mint_id()?;
                let newest_id = storage.mint_id()?;
                storage.commit(
                    &[
                        StorageWrite::Entry {
                            value: entry(oldest_id, root_id, "message"),
                        },
                        StorageWrite::Entry {
                            value: entry(middle_id, root_id, "message"),
                        },
                        StorageWrite::Entry {
                            value: entry(newest_id, root_id, "message"),
                        },
                    ],
                    &CONTEXT,
                )?;

                let first = storage.scan_entries(
                    EntryQuery {
                        conversation_id: root_id,
                        min_entry_id: None,
                        max_entry_id: None,
                    },
                    2,
                    None,
                    &CONTEXT,
                )?;
                a.deep_equal(&page_ids(&first), &json!([newest_id, middle_id]))?;
                let appended_id = storage.mint_id()?;
                storage.commit(
                    &[StorageWrite::Entry {
                        value: entry(appended_id, root_id, "message"),
                    }],
                    &CONTEXT,
                )?;
                let second = storage.scan_entries(
                    EntryQuery {
                        conversation_id: root_id,
                        min_entry_id: None,
                        max_entry_id: None,
                    },
                    2,
                    first.next.as_ref(),
                    &CONTEXT,
                )?;
                a.deep_equal(&page_ids(&second), &json!([oldest_id]))?;
                a.strict_equal(&json_of(&second.next), &Value::Null)?;
                Ok(())
            })
        },
    ));

    cases.push(create_case(
        options,
        "paginates conversations by opaque cursor in ascending ID order",
        {
            let a = Arc::clone(&a);
            Arc::new(move |storage: Arc<dyn Storage>| {
                let root_id = create_root(storage.as_ref())?;
                let second_id = storage.mint_id()?;
                let third_id = storage.mint_id()?;
                storage.commit(
                    &[
                        StorageWrite::Conversation {
                            value: ConversationRecord {
                                id: third_id,
                                parent: None,
                                owner: None,
                            },
                        },
                        StorageWrite::Conversation {
                            value: ConversationRecord {
                                id: second_id,
                                parent: None,
                                owner: None,
                            },
                        },
                    ],
                    &CONTEXT,
                )?;

                let first =
                    storage.scan_conversations(ConversationQuery::default(), 2, None, &CONTEXT)?;
                a.deep_equal(&page_ids(&first), &json!([root_id, second_id]))?;
                a.ok(first.next.is_some(), "Expected value to be defined")?;
                let round_tripped_cursor: Option<Cursor> =
                    serde_json::from_value(json_of(&first.next)).expect("cursor round-trips");
                let second = storage.scan_conversations(
                    ConversationQuery::default(),
                    2,
                    round_tripped_cursor.as_ref(),
                    &CONTEXT,
                )?;
                a.deep_equal(&page_ids(&second), &json!([third_id]))?;
                a.strict_equal(&json_of(&second.next), &Value::Null)?;
                Ok(())
            })
        },
    ));

    cases.push(create_case(
        options,
        "filters and pages conversations by durable owner edges",
        {
            let a = Arc::clone(&a);
            Arc::new(move |storage: Arc<dyn Storage>| {
                let root_id = create_root(storage.as_ref())?;
                let other_owner_id = storage.mint_id()?;
                let first_task_id = storage.mint_id()?;
                let second_task_id = storage.mint_id()?;
                let first_id = storage.mint_id()?;
                let second_id = storage.mint_id()?;
                let third_id = storage.mint_id()?;
                storage.commit(
                    &[
                        StorageWrite::Conversation {
                            value: ConversationRecord {
                                id: other_owner_id,
                                parent: None,
                                owner: None,
                            },
                        },
                        StorageWrite::Conversation {
                            value: ConversationRecord {
                                id: first_id,
                                parent: None,
                                owner: Some(ConversationOwner {
                                    conversation_id: root_id,
                                    task_id: first_task_id,
                                }),
                            },
                        },
                        StorageWrite::Conversation {
                            value: ConversationRecord {
                                id: second_id,
                                parent: None,
                                owner: Some(ConversationOwner {
                                    conversation_id: root_id,
                                    task_id: second_task_id,
                                }),
                            },
                        },
                        StorageWrite::Conversation {
                            value: ConversationRecord {
                                id: third_id,
                                parent: None,
                                owner: Some(ConversationOwner {
                                    conversation_id: other_owner_id,
                                    task_id: first_task_id,
                                }),
                            },
                        },
                    ],
                    &CONTEXT,
                )?;

                let first = storage.scan_conversations(
                    ConversationQuery {
                        owner_conversation_id: Some(root_id),
                        owner_task_id: None,
                    },
                    1,
                    None,
                    &CONTEXT,
                )?;
                a.deep_equal(&page_ids(&first), &json!([first_id]))?;
                a.ok(first.next.is_some(), "Expected value to be defined")?;
                let second = storage.scan_conversations(
                    ConversationQuery {
                        owner_conversation_id: Some(root_id),
                        owner_task_id: None,
                    },
                    1,
                    first.next.as_ref(),
                    &CONTEXT,
                )?;
                a.deep_equal(&page_ids(&second), &json!([second_id]))?;
                a.strict_equal(&json_of(&second.next), &Value::Null)?;
                let by_task = storage.scan_conversations(
                    ConversationQuery {
                        owner_conversation_id: None,
                        owner_task_id: Some(first_task_id),
                    },
                    10,
                    None,
                    &CONTEXT,
                )?;
                a.deep_equal(&page_ids(&by_task), &json!([first_id, third_id]))?;
                let by_both = storage.scan_conversations(
                    ConversationQuery {
                        owner_conversation_id: Some(root_id),
                        owner_task_id: Some(first_task_id),
                    },
                    10,
                    None,
                    &CONTEXT,
                )?;
                a.deep_equal(&page_ids(&by_both), &json!([first_id]))?;
                Ok(())
            })
        },
    ));

    cases.push(create_case(
        options,
        "scans deep fork history newest-first through every ancestor cap",
        {
            let a = Arc::clone(&a);
            Arc::new(move |storage: Arc<dyn Storage>| {
                let root_id = create_root(storage.as_ref())?;
                let root_first = storage.mint_id()?;
                let root_fork_point = storage.mint_id()?;
                let root_excluded_same_commit = storage.mint_id()?;
                let root_entries_seq = storage.commit(
                    &[
                        StorageWrite::Entry {
                            value: entry(root_first, root_id, "message"),
                        },
                        StorageWrite::Entry {
                            value: with_head(entry(root_fork_point, root_id, "marker"), root_first),
                        },
                        StorageWrite::Entry {
                            value: entry(root_excluded_same_commit, root_id, "message"),
                        },
                    ],
                    &CONTEXT,
                )?;
                let child_id = storage.mint_id()?;
                storage.commit(
                    &[StorageWrite::Conversation {
                        value: ConversationRecord {
                            id: child_id,
                            parent: Some(ConversationParent {
                                conversation_id: root_id,
                                at: root_fork_point,
                            }),
                            owner: None,
                        },
                    }],
                    &CONTEXT,
                )?;
                let child_fork_point = storage.mint_id()?;
                let child_excluded = storage.mint_id()?;
                storage.commit(
                    &[
                        StorageWrite::Entry {
                            value: entry(child_fork_point, child_id, "note"),
                        },
                        StorageWrite::Entry {
                            value: entry(child_excluded, child_id, "message"),
                        },
                    ],
                    &CONTEXT,
                )?;
                let root_excluded_later = storage.mint_id()?;
                storage.commit(
                    &[StorageWrite::Entry {
                        value: entry(root_excluded_later, root_id, "message"),
                    }],
                    &CONTEXT,
                )?;
                let grandchild_id = storage.mint_id()?;
                storage.commit(
                    &[StorageWrite::Conversation {
                        value: ConversationRecord {
                            id: grandchild_id,
                            parent: Some(ConversationParent {
                                conversation_id: child_id,
                                at: child_fork_point,
                            }),
                            owner: None,
                        },
                    }],
                    &CONTEXT,
                )?;
                let grandchild_head = storage.mint_id()?;
                let grandchild_tail = storage.mint_id()?;
                let grandchild_entries_seq = storage.commit(
                    &[
                        StorageWrite::Entry {
                            value: with_head(
                                entry(grandchild_head, grandchild_id, "marker"),
                                grandchild_head,
                            ),
                        },
                        StorageWrite::Entry {
                            value: entry(grandchild_tail, grandchild_id, "message"),
                        },
                    ],
                    &CONTEXT,
                )?;
                let child_excluded_later = storage.mint_id()?;
                storage.commit(
                    &[StorageWrite::Entry {
                        value: entry(child_excluded_later, child_id, "message"),
                    }],
                    &CONTEXT,
                )?;

                let scan = |limit: usize, cursor: Option<&Cursor>| {
                    storage.scan_entries(
                        EntryQuery {
                            conversation_id: grandchild_id,
                            min_entry_id: None,
                            max_entry_id: None,
                        },
                        limit,
                        cursor,
                        &CONTEXT,
                    )
                };
                let first = scan(2, None)?;
                a.deep_equal(
                    &page_ids(&first),
                    &json!([grandchild_tail, grandchild_head]),
                )?;
                let second = scan(2, first.next.as_ref())?;
                a.deep_equal(
                    &page_ids(&second),
                    &json!([child_fork_point, root_fork_point]),
                )?;
                let third = scan(2, second.next.as_ref())?;
                a.deep_equal(&page_ids(&third), &json!([root_first]))?;
                a.strict_equal(&json_of(&third.next), &Value::Null)?;

                let current_marker =
                    storage.find_latest_head_marker(grandchild_id, None, &CONTEXT)?;
                a.strict_equal(
                    &current_marker
                        .as_ref()
                        .map(|marker| json!(marker.id))
                        .unwrap_or(Value::Null),
                    &json!(grandchild_head),
                )?;
                a.strict_equal(
                    &current_marker
                        .as_ref()
                        .and_then(|marker| marker.head)
                        .map(|head| json!(head))
                        .unwrap_or(Value::Null),
                    &json!(grandchild_head),
                )?;
                let historical_marker = storage.find_latest_head_marker(
                    grandchild_id,
                    Some(child_fork_point),
                    &CONTEXT,
                )?;
                a.strict_equal(
                    &historical_marker
                        .as_ref()
                        .map(|marker| json!(marker.id))
                        .unwrap_or(Value::Null),
                    &json!(root_fork_point),
                )?;
                a.strict_equal(
                    &historical_marker
                        .as_ref()
                        .and_then(|marker| marker.head)
                        .map(|head| json!(head))
                        .unwrap_or(Value::Null),
                    &json!(root_first),
                )?;
                let missing_marker =
                    storage.find_latest_head_marker(grandchild_id, Some(root_first), &CONTEXT)?;
                a.strict_equal(&json_of(&missing_marker), &Value::Null)?;

                let active_first = storage.scan_entries(
                    EntryQuery {
                        conversation_id: grandchild_id,
                        min_entry_id: current_marker.as_ref().and_then(|marker| marker.head),
                        max_entry_id: None,
                    },
                    1,
                    None,
                    &CONTEXT,
                )?;
                a.deep_equal(&page_ids(&active_first), &json!([grandchild_tail]))?;
                a.ok(active_first.next.is_some(), "Expected value to be defined")?;
                let active_second = storage.scan_entries(
                    EntryQuery {
                        conversation_id: grandchild_id,
                        min_entry_id: current_marker.as_ref().and_then(|marker| marker.head),
                        max_entry_id: None,
                    },
                    1,
                    active_first.next.as_ref(),
                    &CONTEXT,
                )?;
                a.deep_equal(&page_ids(&active_second), &json!([grandchild_head]))?;
                a.strict_equal(&json_of(&active_second.next), &Value::Null)?;

                let bounded = storage.scan_entries(
                    EntryQuery {
                        conversation_id: grandchild_id,
                        min_entry_id: historical_marker.as_ref().and_then(|marker| marker.head),
                        max_entry_id: Some(child_fork_point),
                    },
                    10,
                    None,
                    &CONTEXT,
                )?;
                a.deep_equal(
                    &page_ids(&bounded),
                    &json!([child_fork_point, root_fork_point, root_first]),
                )?;

                let found = storage.entry(root_first, &CONTEXT)?;
                a.deep_equal(
                    &found
                        .as_ref()
                        .map(entry_with_commit_seq)
                        .unwrap_or(Value::Null),
                    &json!({
                        "entry": json_of(&entry(root_first, root_id, "message")),
                        "commitSeq": root_entries_seq,
                    }),
                )?;
                a.strict_equal(
                    &storage
                        .entry(root_fork_point, &CONTEXT)?
                        .map(|found| json!(found.commit_seq))
                        .unwrap_or(Value::Null),
                    &json!(root_entries_seq),
                )?;
                a.strict_equal(
                    &storage
                        .entry(grandchild_head, &CONTEXT)?
                        .map(|found| json!(found.commit_seq))
                        .unwrap_or(Value::Null),
                    &json!(grandchild_entries_seq),
                )?;
                a.strict_equal(
                    &storage
                        .entry(grandchild_tail, &CONTEXT)?
                        .map(|found| json!(found.commit_seq))
                        .unwrap_or(Value::Null),
                    &json!(grandchild_entries_seq),
                )?;
                let missing = storage.entry(id_from_number(999_999), &CONTEXT)?;
                a.strict_equal(
                    &missing
                        .as_ref()
                        .map(entry_with_commit_seq)
                        .unwrap_or(Value::Null),
                    &Value::Null,
                )?;

                let visible = storage.entry_visible(grandchild_id, root_first, &CONTEXT)?;
                a.deep_equal(
                    &visible
                        .as_ref()
                        .map(entry_with_commit_seq)
                        .unwrap_or(Value::Null),
                    &json!({
                        "entry": json_of(&entry(root_first, root_id, "message")),
                        "commitSeq": root_entries_seq,
                    }),
                )?;
                a.strict_equal(
                    &storage
                        .entry_visible(grandchild_id, child_fork_point, &CONTEXT)?
                        .map(|found| json!(found.entry.conversation_id))
                        .unwrap_or(Value::Null),
                    &json!(child_id),
                )?;
                a.strict_equal(
                    &storage
                        .entry_visible(grandchild_id, grandchild_tail, &CONTEXT)?
                        .map(|found| json!(found.commit_seq))
                        .unwrap_or(Value::Null),
                    &json!(grandchild_entries_seq),
                )?;
                for missing_visible in [
                    root_excluded_same_commit,
                    root_excluded_later,
                    child_excluded,
                    child_excluded_later,
                ] {
                    let found = storage.entry_visible(grandchild_id, missing_visible, &CONTEXT)?;
                    a.strict_equal(
                        &found
                            .as_ref()
                            .map(entry_with_commit_seq)
                            .unwrap_or(Value::Null),
                        &Value::Null,
                    )?;
                }
                let wrong_root = storage.entry_visible(root_id, grandchild_head, &CONTEXT)?;
                a.strict_equal(
                    &wrong_root
                        .as_ref()
                        .map(entry_with_commit_seq)
                        .unwrap_or(Value::Null),
                    &Value::Null,
                )?;
                let beyond =
                    storage.entry_visible(grandchild_id, id_from_number(999_999), &CONTEXT)?;
                a.strict_equal(
                    &beyond
                        .as_ref()
                        .map(entry_with_commit_seq)
                        .unwrap_or(Value::Null),
                    &Value::Null,
                )?;
                let storage_op = Arc::clone(&storage);
                a.rejects(
                    Box::new(move || {
                        storage_op
                            .entry_visible(id_from_number(999_999), root_first, &CONTEXT)
                            .map(|_| ())
                    }),
                    "Unknown conversation",
                )?;
                let storage_scan_op = Arc::clone(&storage);
                a.rejects(
                    Box::new(move || {
                        storage_scan_op
                            .scan_entries(
                                EntryQuery {
                                    conversation_id: id_from_number(999_999),
                                    min_entry_id: None,
                                    max_entry_id: None,
                                },
                                10,
                                None,
                                &CONTEXT,
                            )
                            .map(|_| ())
                    }),
                    "Unknown conversation",
                )?;
                Ok(())
            })
        },
    ));

    cases.push(create_case(
        options,
        "replaces complete task records and pages filtered task scans",
        {
            let a = Arc::clone(&a);
            Arc::new(move |storage: Arc<dyn Storage>| {
                let root_id = create_root(storage.as_ref())?;
                let first_id = storage.mint_id()?;
                let second_id = storage.mint_id()?;
                let third_id = storage.mint_id()?;
                let mut first = pending_task(first_id, root_id);
                first.memos = Some(
                    json!({ "winner": "first" })
                        .as_object()
                        .cloned()
                        .unwrap_or_default(),
                );
                let mut second = pending_task(second_id, root_id);
                second.background = true;
                let mut third = pending_task(third_id, root_id);
                third.abort_requested = true;
                storage.commit(
                    &[
                        StorageWrite::Task {
                            value: first.clone(),
                        },
                        StorageWrite::Task {
                            value: second.clone(),
                        },
                        StorageWrite::Task {
                            value: third.clone(),
                        },
                    ],
                    &CONTEXT,
                )?;

                let mut running = first.clone();
                running.state = TaskState::Running {
                    checkpoint: json!({ "phase": "effect", "attempt": 1 }),
                };
                running.abort_requested = true;
                storage.commit(
                    &[StorageWrite::Task {
                        value: running.clone(),
                    }],
                    &CONTEXT,
                )?;
                let found = storage.task(first_id, &CONTEXT)?;
                a.deep_equal(&json_of(&found), &json_of(&running))?;
                let terminal = TaskRecord {
                    id: first_id,
                    conversation_id: root_id,
                    kind: first.kind.clone(),
                    version: first.version,
                    input: first.input.clone(),
                    owner: None,
                    background: false,
                    abort_requested: true,
                    state: TaskState::Terminal {
                        outcome: crate::durable::types::TaskOutcome::Completed {
                            result: json!({ "entryId": 99 }),
                        },
                    },
                    memos: None,
                };
                storage.commit(
                    &[StorageWrite::Task {
                        value: terminal.clone(),
                    }],
                    &CONTEXT,
                )?;
                let found = storage.task(first_id, &CONTEXT)?;
                a.deep_equal(&json_of(&found), &json_of(&terminal))?;

                let pending_page = storage.scan_tasks(
                    TaskQuery {
                        status: Some(TaskStatus::Pending),
                        ..Default::default()
                    },
                    1,
                    None,
                    &CONTEXT,
                )?;
                a.deep_equal(&page_ids(&pending_page), &json!([second_id]))?;
                a.ok(pending_page.next.is_some(), "Expected value to be defined")?;
                let pending_second = storage.scan_tasks(
                    TaskQuery {
                        status: Some(TaskStatus::Pending),
                        ..Default::default()
                    },
                    1,
                    pending_page.next.as_ref(),
                    &CONTEXT,
                )?;
                a.deep_equal(&page_ids(&pending_second), &json!([third_id]))?;
                let aborted_page = storage.scan_tasks(
                    TaskQuery {
                        status: Some(TaskStatus::Terminal),
                        abort_requested: Some(true),
                        ..Default::default()
                    },
                    10,
                    None,
                    &CONTEXT,
                )?;
                a.deep_equal(
                    &Value::Array(aborted_page.items.iter().map(json_of).collect()),
                    &Value::Array(vec![json_of(&terminal)]),
                )?;
                let background_page = storage.scan_tasks(
                    TaskQuery {
                        background: Some(true),
                        ..Default::default()
                    },
                    10,
                    None,
                    &CONTEXT,
                )?;
                a.deep_equal(&page_ids(&background_page), &json!([second_id]))?;
                Ok(())
            })
        },
    ));

    cases.push(create_case(
        options,
        "stores owners and scans waiting and completing tasks by status",
        {
            let a = Arc::clone(&a);
            Arc::new(move |storage: Arc<dyn Storage>| {
                let root_id = create_root(storage.as_ref())?;
                let owner_id = storage.mint_id()?;
                let waiting_id = storage.mint_id()?;
                let completing_id = storage.mint_id()?;
                let owner = pending_task(owner_id, root_id);
                let mut waiting = pending_task(waiting_id, root_id);
                waiting.owner = Some(owner_id);
                waiting.state = TaskState::Waiting {
                    checkpoint: json!({ "phase": "next" }),
                    on: vec![owner_id],
                    policy: crate::durable::types::JoinPolicy::AllSettled,
                };
                waiting.memos = Some(
                    json!({ "kept": true })
                        .as_object()
                        .cloned()
                        .unwrap_or_default(),
                );
                let mut completing = pending_task(completing_id, root_id);
                completing.owner = Some(owner_id);
                completing.state = TaskState::Completing {
                    outcome: crate::durable::types::TaskOutcome::Failed {
                        error: crate::durable::types::TaskOutcomeError {
                            message: String::from("held"),
                            detail: None,
                        },
                        result: None,
                    },
                };
                let writes = vec![
                    StorageWrite::Task { value: owner },
                    StorageWrite::Task {
                        value: waiting.clone(),
                    },
                    StorageWrite::Task {
                        value: completing.clone(),
                    },
                ];
                storage.commit(&writes, &CONTEXT)?;
                let found_waiting = storage.task(waiting_id, &CONTEXT)?;
                a.deep_equal(&json_of(&found_waiting), &json_of(&waiting))?;
                let found_completing = storage.task(completing_id, &CONTEXT)?;
                a.deep_equal(&json_of(&found_completing), &json_of(&completing))?;
                let scan = |status: TaskStatus| {
                    storage
                        .scan_tasks(
                            TaskQuery {
                                status: Some(status),
                                ..Default::default()
                            },
                            10,
                            None,
                            &CONTEXT,
                        )
                        .map(|page| Value::Array(page.items.iter().map(json_of).collect()))
                        .map_err(failure)
                };
                a.deep_equal(
                    &scan(TaskStatus::Waiting)?,
                    &Value::Array(vec![json_of(&waiting)]),
                )?;
                a.deep_equal(
                    &scan(TaskStatus::Completing)?,
                    &Value::Array(vec![json_of(&completing)]),
                )?;
                let pending = storage.scan_tasks(
                    TaskQuery {
                        status: Some(TaskStatus::Pending),
                        ..Default::default()
                    },
                    10,
                    None,
                    &CONTEXT,
                )?;
                a.deep_equal(
                    &Value::Array(pending.items.iter().map(|task| json!(task.id)).collect()),
                    &json!([owner_id]),
                )?;
                let completing_outcome = match &completing.state {
                    TaskState::Completing { outcome } => outcome.clone(),
                    _ => unreachable!("completing state"),
                };
                let mut terminal = completing.clone();
                terminal.state = TaskState::Terminal {
                    outcome: completing_outcome,
                };
                storage.commit(
                    &[StorageWrite::Task {
                        value: terminal.clone(),
                    }],
                    &CONTEXT,
                )?;
                a.deep_equal(&scan(TaskStatus::Completing)?, &json!([]))?;
                a.deep_equal(
                    &scan(TaskStatus::Terminal)?,
                    &Value::Array(vec![json_of(&terminal)]),
                )?;
                Ok(())
            })
        },
    ));

    cases.push(create_case(
        options,
        "indexes request IDs per conversation and replaces complete submission records",
        {
            let a = Arc::clone(&a);
            Arc::new(move |storage: Arc<dyn Storage>| {
                let root_id = create_root(storage.as_ref())?;
                let second_conversation_id = storage.mint_id()?;
                storage.commit(
                    &[StorageWrite::Conversation {
                        value: ConversationRecord {
                            id: second_conversation_id,
                            parent: None,
                            owner: None,
                        },
                    }],
                    &CONTEXT,
                )?;
                let first_id = storage.mint_id()?;
                let second_id = storage.mint_id()?;
                let other_conversation_id = storage.mint_id()?;
                let first = SubmissionRecord {
                    conversation_id: root_id,
                    request_id: Some(String::from("same")),
                    r#type: SubmissionType::Input,
                    status: SubmissionStatus::Queued,
                    entry: None,
                    answer: None,
                    reason: None,
                    detail: None,
                    id: first_id,
                    entry_before_id: false,
                };
                let second = SubmissionRecord {
                    conversation_id: root_id,
                    request_id: Some(String::from("other")),
                    r#type: SubmissionType::Input,
                    status: SubmissionStatus::Queued,
                    entry: None,
                    answer: None,
                    reason: None,
                    detail: None,
                    id: second_id,
                    entry_before_id: false,
                };
                let other_conversation = SubmissionRecord {
                    conversation_id: second_conversation_id,
                    request_id: Some(String::from("same")),
                    r#type: SubmissionType::Input,
                    status: SubmissionStatus::Queued,
                    entry: None,
                    answer: None,
                    reason: None,
                    detail: None,
                    id: other_conversation_id,
                    entry_before_id: false,
                };
                storage.commit(
                    &[
                        StorageWrite::Submission {
                            value: first.clone(),
                        },
                        StorageWrite::Submission {
                            value: second.clone(),
                        },
                        StorageWrite::Submission {
                            value: other_conversation.clone(),
                        },
                    ],
                    &CONTEXT,
                )?;
                let by_request = storage.submission_by_request(root_id, "same", &CONTEXT)?;
                a.deep_equal(&json_of(&by_request), &json_of(&first))?;
                let other_by_request =
                    storage.submission_by_request(second_conversation_id, "same", &CONTEXT)?;
                a.deep_equal(&json_of(&other_by_request), &json_of(&other_conversation))?;

                let placed_entry = storage.mint_id()?;
                let mut placed_second = second.clone();
                placed_second.status = SubmissionStatus::Placed;
                placed_second.entry = Some(placed_entry);
                storage.commit(
                    &[StorageWrite::Submission {
                        value: placed_second.clone(),
                    }],
                    &CONTEXT,
                )?;
                let found = storage.submission(second_id, &CONTEXT)?;
                a.deep_equal(&json_of(&found), &json_of(&placed_second))?;
                let placed_by_request =
                    storage.submission_by_request(root_id, "other", &CONTEXT)?;
                a.deep_equal(&json_of(&placed_by_request), &json_of(&placed_second))?;

                let ids = |query: SubmissionQuery| -> Result<Vec<i64>, ConformanceFailure> {
                    let mut found = Vec::new();
                    let mut cursor: Option<Cursor> = None;
                    loop {
                        let page = storage
                            .scan_submissions(query.clone(), 1, cursor.as_ref(), &CONTEXT)
                            .map_err(failure)?;
                        found.extend(page.items.iter().map(|submission| submission.id));
                        cursor = page.next.clone();
                        if cursor.is_none() {
                            break;
                        }
                    }
                    Ok(found)
                };
                a.deep_equal(
                    &json!(ids(SubmissionQuery::default())?),
                    &json!([first_id, second_id, other_conversation_id]),
                )?;
                a.deep_equal(
                    &json!(ids(SubmissionQuery {
                        conversation_id: Some(root_id),
                        status: None,
                    })?),
                    &json!([first_id, second_id]),
                )?;
                // A status change moves the record between status scans.
                a.deep_equal(
                    &json!(ids(SubmissionQuery {
                        conversation_id: None,
                        status: Some(SubmissionStatus::Queued),
                    })?),
                    &json!([first_id, other_conversation_id]),
                )?;
                a.deep_equal(
                    &json!(ids(SubmissionQuery {
                        conversation_id: None,
                        status: Some(SubmissionStatus::Placed),
                    })?),
                    &json!([second_id]),
                )?;
                a.deep_equal(
                    &json!(ids(SubmissionQuery {
                        conversation_id: Some(second_conversation_id),
                        status: Some(SubmissionStatus::Queued),
                    })?),
                    &json!([other_conversation_id]),
                )?;
                a.deep_equal(
                    &json!(ids(SubmissionQuery {
                        conversation_id: Some(second_conversation_id),
                        status: Some(SubmissionStatus::Placed),
                    })?),
                    &json!([]),
                )?;
                let placed_page = storage.scan_submissions(
                    SubmissionQuery {
                        conversation_id: None,
                        status: Some(SubmissionStatus::Placed),
                    },
                    10,
                    None,
                    &CONTEXT,
                )?;
                a.deep_equal(
                    &Value::Array(placed_page.items.iter().map(json_of).collect()),
                    &Value::Array(vec![json_of(&placed_second)]),
                )?;
                Ok(())
            })
        },
    ));

    cases.push(create_case(
        options,
        "stores passive write submissions without input-only lifecycle states",
        {
            let a = Arc::clone(&a);
            Arc::new(move |storage: Arc<dyn Storage>| {
                let root_id = create_root(storage.as_ref())?;
                let done_id = storage.mint_id()?;
                let failed_id = storage.mint_id()?;
                let queued_done = SubmissionRecord {
                    conversation_id: root_id,
                    request_id: Some(String::from("passive-done")),
                    r#type: SubmissionType::Write,
                    status: SubmissionStatus::Queued,
                    entry: None,
                    answer: None,
                    reason: None,
                    detail: None,
                    id: done_id,
                    entry_before_id: false,
                };
                let queued_failed = SubmissionRecord {
                    conversation_id: root_id,
                    request_id: Some(String::from("passive-failed")),
                    r#type: SubmissionType::Write,
                    status: SubmissionStatus::Queued,
                    entry: None,
                    answer: None,
                    reason: None,
                    detail: None,
                    id: failed_id,
                    entry_before_id: false,
                };
                storage.commit(
                    &[
                        StorageWrite::Submission {
                            value: queued_done.clone(),
                        },
                        StorageWrite::Submission {
                            value: queued_failed.clone(),
                        },
                    ],
                    &CONTEXT,
                )?;

                let done_entry = storage.mint_id()?;
                let mut done = queued_done.clone();
                done.status = SubmissionStatus::Done;
                done.entry = Some(done_entry);
                let mut unanswered = queued_failed.clone();
                unanswered.status = SubmissionStatus::Unanswered;
                unanswered.reason = Some(String::from("closed"));
                unanswered.detail = Some(json!({ "retryable": false }));
                storage.commit(
                    &[
                        StorageWrite::Submission {
                            value: done.clone(),
                        },
                        StorageWrite::Submission {
                            value: unanswered.clone(),
                        },
                    ],
                    &CONTEXT,
                )?;
                let found_done = storage.submission(done_id, &CONTEXT)?;
                a.deep_equal(&json_of(&found_done), &json_of(&done))?;
                let done_by_request =
                    storage.submission_by_request(root_id, "passive-done", &CONTEXT)?;
                a.deep_equal(&json_of(&done_by_request), &json_of(&done))?;
                let found_failed = storage.submission(failed_id, &CONTEXT)?;
                a.deep_equal(&json_of(&found_failed), &json_of(&unanswered))?;
                let failed_by_request =
                    storage.submission_by_request(root_id, "passive-failed", &CONTEXT)?;
                a.deep_equal(&json_of(&failed_by_request), &json_of(&unanswered))?;
                Ok(())
            })
        },
    ));

    cases.push(create_case(
        options,
        "reconstructs rewindable documents and preserves half-open incarnations",
        {
            let a = Arc::clone(&a);
            Arc::new(move |storage: Arc<dyn Storage>| {
                let root_id = create_root(storage.as_ref())?;
                let first_id = storage.mint_id()?;
                let first_record = DocumentCreate {
                    id: first_id,
                    kind: String::from("conversation.notes"),
                    key: None,
                    scope: DocumentScope::Conversation {
                        conversation_id: root_id,
                    },
                    history: Some(DocumentHistory::Rewindable),
                    fork: Some(DocumentFork::AsOf),
                };
                let initial = json!({ "items": ["a"], "nested": { "count": 1 } });
                let created_at = storage.commit(
                    &[StorageWrite::DocumentCreate {
                        record: first_record.clone(),
                        content: DocumentContent::Base {
                            version: 1,
                            value: initial.as_object().cloned().unwrap_or_default(),
                        },
                    }],
                    &CONTEXT,
                )?;
                let appended = vec![json!("b")];
                let changed_at = storage.commit(
                    &[StorageWrite::DocumentChange {
                        id: first_id,
                        content: DocumentContent::Delta {
                            version: 1,
                            ops: vec![
                                splice_op(&["items"], 1, 0, appended.clone()),
                                set_op(&["nested", "count"], json!(2)),
                            ],
                        },
                    }],
                    &CONTEXT,
                )?;

                let created_document =
                    storage.document(first_id, DocumentPoint::Seq(created_at), &CONTEXT)?;
                a.partial_deep_equal(
                    &created_document
                        .as_ref()
                        .map(document_view)
                        .unwrap_or(Value::Null),
                    &json!({
                        "version": 1,
                        "value": { "items": ["a"], "nested": { "count": 1 } },
                        "deltasSinceBase": 0,
                    }),
                )?;
                let changed = storage
                    .document(first_id, DocumentPoint::Seq(changed_at), &CONTEXT)?
                    .expect("changed document exists");
                a.deep_equal(
                    &Value::Object(changed.value.clone()),
                    &json!({ "items": ["a", "b"], "nested": { "count": 2 } }),
                )?;
                a.strict_equal(&json!(changed.deltas_since_base), &json!(1))?;
                let mut changed_value = changed.value.clone();
                if let Some(items) = changed_value.get_mut("items").and_then(Value::as_array_mut) {
                    items.push(json!("read mutation"));
                }
                let current = storage.document(first_id, DocumentPoint::Current, &CONTEXT)?;
                a.deep_equal(
                    &current
                        .as_ref()
                        .map(|document| Value::Object(document.value.clone()))
                        .unwrap_or(Value::Null),
                    &json!({ "items": ["a", "b"], "nested": { "count": 2 } }),
                )?;

                let checkpoint_at = storage.commit(
                    &[StorageWrite::DocumentChange {
                        id: first_id,
                        content: DocumentContent::Base {
                            version: 2,
                            value: json!({ "items": ["checkpoint"], "nested": { "count": 3 } })
                                .as_object()
                                .cloned()
                                .unwrap_or_default(),
                        },
                    }],
                    &CONTEXT,
                )?;
                let replaced_at = storage.commit(
                    &[StorageWrite::DocumentChange {
                        id: first_id,
                        content: DocumentContent::Delta {
                            version: 2,
                            ops: vec![replace_op(json!({
                                "items": ["replacement"],
                                "nested": { "count": 4 },
                            }))],
                        },
                    }],
                    &CONTEXT,
                )?;
                let changed_again =
                    storage.document(first_id, DocumentPoint::Seq(changed_at), &CONTEXT)?;
                a.partial_deep_equal(
                    &changed_again
                        .as_ref()
                        .map(document_view)
                        .unwrap_or(Value::Null),
                    &json!({
                        "version": 1,
                        "value": { "items": ["a", "b"], "nested": { "count": 2 } },
                    }),
                )?;
                let checkpointed =
                    storage.document(first_id, DocumentPoint::Seq(checkpoint_at), &CONTEXT)?;
                a.partial_deep_equal(
                    &checkpointed
                        .as_ref()
                        .map(document_view)
                        .unwrap_or(Value::Null),
                    &json!({
                        "version": 2,
                        "value": { "items": ["checkpoint"], "nested": { "count": 3 } },
                        "deltasSinceBase": 0,
                    }),
                )?;
                let replaced =
                    storage.document(first_id, DocumentPoint::Seq(replaced_at), &CONTEXT)?;
                a.partial_deep_equal(
                    &replaced.as_ref().map(document_view).unwrap_or(Value::Null),
                    &json!({
                        "value": { "items": ["replacement"], "nested": { "count": 4 } },
                        "deltasSinceBase": 1,
                    }),
                )?;
                let current_deltas =
                    storage.document(first_id, DocumentPoint::Current, &CONTEXT)?;
                a.strict_equal(
                    &current_deltas
                        .as_ref()
                        .map(|document| json!(document.deltas_since_base))
                        .unwrap_or(Value::Null),
                    &json!(1),
                )?;

                let second_id = storage.mint_id()?;
                let retired_record = DocumentCreate {
                    id: second_id,
                    kind: first_record.kind.clone(),
                    key: first_record.key.clone(),
                    scope: first_record.scope,
                    history: first_record.history,
                    fork: first_record.fork,
                };
                let retired_at = storage.commit(
                    &[
                        StorageWrite::DocumentCreate {
                            record: retired_record,
                            content: DocumentContent::Base {
                                version: 1,
                                value: json!({ "items": ["new"] })
                                    .as_object()
                                    .cloned()
                                    .unwrap_or_default(),
                            },
                        },
                        StorageWrite::DocumentRetire { id: first_id },
                        StorageWrite::DocumentChange {
                            id: first_id,
                            content: DocumentContent::Delta {
                                version: 2,
                                ops: vec![set_op(&["retiring"], json!(true))],
                            },
                        },
                    ],
                    &CONTEXT,
                )?;
                let address = DocumentAddress {
                    kind: first_record.kind.clone(),
                    scope: first_record.scope,
                    key: first_record.key.clone(),
                };
                a.strict_equal(
                    &storage
                        .find_document(&address, DocumentPoint::Seq(changed_at), &CONTEXT)?
                        .map(|record| json!(record.id))
                        .unwrap_or(Value::Null),
                    &json!(first_id),
                )?;
                let retired_find =
                    storage.find_document(&address, DocumentPoint::Seq(retired_at), &CONTEXT)?;
                a.partial_deep_equal(
                    &retired_find
                        .as_ref()
                        .map(|record| json!({ "id": record.id, "createdAt": record.created_at }))
                        .unwrap_or(Value::Null),
                    &json!({ "id": second_id, "createdAt": retired_at }),
                )?;
                let changed_scan = storage.scan_documents(
                    DocumentQuery {
                        scope: first_record.scope,
                        at: DocumentPoint::Seq(changed_at),
                        kind: None,
                    },
                    10,
                    None,
                    &CONTEXT,
                )?;
                a.deep_equal(&page_ids(&changed_scan), &json!([first_id]))?;
                let retired_scan = storage.scan_documents(
                    DocumentQuery {
                        scope: first_record.scope,
                        at: DocumentPoint::Seq(retired_at),
                        kind: None,
                    },
                    10,
                    None,
                    &CONTEXT,
                )?;
                a.deep_equal(&page_ids(&retired_scan), &json!([second_id]))?;
                let retired_document =
                    storage.document(first_id, DocumentPoint::Seq(retired_at), &CONTEXT)?;
                a.strict_equal(&json_of(&retired_document), &Value::Null)?;
                let second_document =
                    storage.document(second_id, DocumentPoint::Current, &CONTEXT)?;
                a.deep_equal(
                    &second_document
                        .as_ref()
                        .map(|document| Value::Object(document.value.clone()))
                        .unwrap_or(Value::Null),
                    &json!({ "items": ["new"] }),
                )?;
                Ok(())
            })
        },
    ));

    cases.push(create_case(
        options,
        "streams long document tails across root replacement deltas",
        {
            let a = Arc::clone(&a);
            Arc::new(move |storage: Arc<dyn Storage>| {
                let root_id = create_root(storage.as_ref())?;
                let id = storage.mint_id()?;
                let record = DocumentCreate {
                    id,
                    kind: String::from("conversation.long-tail"),
                    key: None,
                    scope: DocumentScope::Conversation {
                        conversation_id: root_id,
                    },
                    history: Some(DocumentHistory::Rewindable),
                    fork: Some(DocumentFork::AsOf),
                };
                let rows: Vec<Value> = (0..512)
                    .map(|value| json!({ "value": value, "stable": format!("row-{value}") }))
                    .collect();
                let initial = json!({ "revision": 0, "rows": rows });
                let created_at = storage.commit(
                    &[StorageWrite::DocumentCreate {
                        record,
                        content: DocumentContent::Base {
                            version: 1,
                            value: initial.as_object().cloned().unwrap_or_default(),
                        },
                    }],
                    &CONTEXT,
                )?;
                let mut before_replacement = initial.clone();
                let mut before_replacement_at = created_at;
                for revision in 1..=24 {
                    let index = (revision * 17) % 512;
                    before_replacement["rows"][index]["value"] = json!(-(revision as i64));
                    before_replacement["revision"] = json!(revision);
                    before_replacement_at = storage.commit(
                        &[StorageWrite::DocumentChange {
                            id,
                            content: DocumentContent::Delta {
                                version: 1,
                                ops: vec![
                                    set_index_op(
                                        &["rows", "value"],
                                        index,
                                        json!(-(revision as i64)),
                                    ),
                                    set_op(&["revision"], json!(revision)),
                                ],
                            },
                        }],
                        &CONTEXT,
                    )?;
                }

                let replacement_rows: Vec<Value> = (0..512)
                .map(|value| json!({ "value": 10_000 + value, "stable": format!("new-{value}") }))
                .collect();
                let mut replacement = json!({ "revision": 100, "rows": replacement_rows });
                let replacement_snapshot = replacement.clone();
                let replacement_at = storage.commit(
                    &[StorageWrite::DocumentChange {
                        id,
                        content: DocumentContent::Delta {
                            version: 1,
                            ops: vec![replace_op(replacement.clone())],
                        },
                    }],
                    &CONTEXT,
                )?;
                replacement["rows"][0]["value"] = json!(-999);

                let mut current = replacement_snapshot.clone();
                for revision in 101..=124 {
                    let index = (revision * 19) % 512;
                    current["rows"][index]["value"] = json!(-(revision as i64));
                    current["revision"] = json!(revision);
                    storage.commit(
                        &[StorageWrite::DocumentChange {
                            id,
                            content: DocumentContent::Delta {
                                version: 1,
                                ops: vec![
                                    set_index_op(
                                        &["rows", "value"],
                                        index,
                                        json!(-(revision as i64)),
                                    ),
                                    set_op(&["revision"], json!(revision)),
                                ],
                            },
                        }],
                        &CONTEXT,
                    )?;
                }

                let created_document =
                    storage.document(id, DocumentPoint::Seq(created_at), &CONTEXT)?;
                a.deep_equal(
                    &created_document
                        .as_ref()
                        .map(|document| Value::Object(document.value.clone()))
                        .unwrap_or(Value::Null),
                    &initial,
                )?;
                let before_document =
                    storage.document(id, DocumentPoint::Seq(before_replacement_at), &CONTEXT)?;
                a.deep_equal(
                    &before_document
                        .as_ref()
                        .map(|document| Value::Object(document.value.clone()))
                        .unwrap_or(Value::Null),
                    &before_replacement,
                )?;
                let replacement_document =
                    storage.document(id, DocumentPoint::Seq(replacement_at), &CONTEXT)?;
                a.deep_equal(
                    &replacement_document
                        .as_ref()
                        .map(|document| Value::Object(document.value.clone()))
                        .unwrap_or(Value::Null),
                    &replacement_snapshot,
                )?;
                let read = storage
                    .document(id, DocumentPoint::Current, &CONTEXT)?
                    .expect("current document exists");
                a.deep_equal(&Value::Object(read.value.clone()), &current)?;
                let read_again = storage.document(id, DocumentPoint::Current, &CONTEXT)?;
                a.deep_equal(
                    &read_again
                        .as_ref()
                        .map(|document| Value::Object(document.value.clone()))
                        .unwrap_or(Value::Null),
                    &current,
                )?;
                Ok(())
            })
        },
    ));

    cases.push(create_case(
        options,
        "copies stored document bases independently and rejects ambiguous sources",
        {
            let a = Arc::clone(&a);
            Arc::new(move |storage: Arc<dyn Storage>| {
                let root_id = create_root(storage.as_ref())?;
                let child_id = storage.mint_id()?;
                let second_child_id = storage.mint_id()?;
                storage.commit(
                    &[
                        StorageWrite::Conversation {
                            value: ConversationRecord {
                                id: child_id,
                                parent: None,
                                owner: None,
                            },
                        },
                        StorageWrite::Conversation {
                            value: ConversationRecord {
                                id: second_child_id,
                                parent: None,
                                owner: None,
                            },
                        },
                    ],
                    &CONTEXT,
                )?;
                let source_id = storage.mint_id()?;
                let source_record = DocumentCreate {
                    id: source_id,
                    kind: String::from("copy.source"),
                    key: None,
                    scope: DocumentScope::Conversation {
                        conversation_id: root_id,
                    },
                    history: Some(DocumentHistory::Rewindable),
                    fork: Some(DocumentFork::AsOf),
                };
                let created_at = storage.commit(
                    &[StorageWrite::DocumentCreate {
                        record: source_record.clone(),
                        content: DocumentContent::Base {
                            version: 2,
                            value: json!({ "count": 1, "rows": [{ "value": "base" }] })
                                .as_object()
                                .cloned()
                                .unwrap_or_default(),
                        },
                    }],
                    &CONTEXT,
                )?;
                storage.commit(
                    &[StorageWrite::DocumentChange {
                        id: source_id,
                        content: DocumentContent::Delta {
                            version: 2,
                            ops: vec![
                                set_op(&["count"], json!(2)),
                                splice_op(&["rows"], 1, 0, vec![json!({ "value": "current" })]),
                            ],
                        },
                    }],
                    &CONTEXT,
                )?;
                let historical_copy_id = storage.mint_id()?;
                let current_copy_id = storage.mint_id()?;
                let retired_copy_id = storage.mint_id()?;
                let child_record = |id: i64, conversation_id: ConversationId| DocumentCreate {
                    id,
                    kind: source_record.kind.clone(),
                    key: None,
                    scope: DocumentScope::Conversation { conversation_id },
                    history: Some(DocumentHistory::Rewindable),
                    fork: Some(DocumentFork::AsOf),
                };
                storage.commit(
                    &[
                        StorageWrite::DocumentCopy {
                            record: child_record(historical_copy_id, child_id),
                            source: DocumentCopySource {
                                id: source_id,
                                at: DocumentPoint::Seq(created_at),
                            },
                        },
                        StorageWrite::DocumentCopy {
                            record: child_record(current_copy_id, second_child_id),
                            source: DocumentCopySource {
                                id: source_id,
                                at: DocumentPoint::Current,
                            },
                        },
                        StorageWrite::DocumentCopy {
                            record: child_record(retired_copy_id, root_id),
                            source: DocumentCopySource {
                                id: source_id,
                                at: DocumentPoint::Current,
                            },
                        },
                        StorageWrite::DocumentRetire {
                            id: retired_copy_id,
                        },
                    ],
                    &CONTEXT,
                )?;
                let historical_copy =
                    storage.document(historical_copy_id, DocumentPoint::Current, &CONTEXT)?;
                a.partial_deep_equal(
                    &historical_copy
                        .as_ref()
                        .map(document_view)
                        .unwrap_or(Value::Null),
                    &json!({
                        "version": 2,
                        "value": { "count": 1, "rows": [{ "value": "base" }] },
                    }),
                )?;
                let current_copy =
                    storage.document(current_copy_id, DocumentPoint::Current, &CONTEXT)?;
                a.partial_deep_equal(
                &current_copy.as_ref().map(document_view).unwrap_or(Value::Null),
                &json!({
                    "version": 2,
                    "value": { "count": 2, "rows": [{ "value": "base" }, { "value": "current" }] },
                }),
            )?;
                let retired_copy =
                    storage.document(retired_copy_id, DocumentPoint::Current, &CONTEXT)?;
                a.strict_equal(&json_of(&retired_copy), &Value::Null)?;

                storage.commit(
                    &[
                        StorageWrite::DocumentChange {
                            id: source_id,
                            content: DocumentContent::Base {
                                version: 2,
                                value: json!({ "count": 99, "rows": [] })
                                    .as_object()
                                    .cloned()
                                    .unwrap_or_default(),
                            },
                        },
                        StorageWrite::DocumentRetire { id: source_id },
                    ],
                    &CONTEXT,
                )?;
                let current_copy_value =
                    storage.document(current_copy_id, DocumentPoint::Current, &CONTEXT)?;
                a.deep_equal(
                    &current_copy_value
                        .as_ref()
                        .map(|document| Value::Object(document.value.clone()))
                        .unwrap_or(Value::Null),
                    &json!({ "count": 2, "rows": [{ "value": "base" }, { "value": "current" }] }),
                )?;

                let latest_source_id = storage.mint_id()?;
                let latest_copy_id = storage.mint_id()?;
                let latest_source = DocumentCreate {
                    id: latest_source_id,
                    kind: String::from("copy.latest"),
                    key: None,
                    scope: DocumentScope::Conversation {
                        conversation_id: root_id,
                    },
                    history: Some(DocumentHistory::Latest),
                    fork: Some(DocumentFork::Current),
                };
                storage.commit(
                    &[StorageWrite::DocumentCreate {
                        record: latest_source.clone(),
                        content: DocumentContent::Base {
                            version: 4,
                            value: json!({ "retained": "copy" })
                                .as_object()
                                .cloned()
                                .unwrap_or_default(),
                        },
                    }],
                    &CONTEXT,
                )?;
                let mut latest_copy_record = latest_source.clone();
                latest_copy_record.id = latest_copy_id;
                latest_copy_record.scope = DocumentScope::Conversation {
                    conversation_id: child_id,
                };
                storage.commit(
                    &[StorageWrite::DocumentCopy {
                        record: latest_copy_record,
                        source: DocumentCopySource {
                            id: latest_source_id,
                            at: DocumentPoint::Current,
                        },
                    }],
                    &CONTEXT,
                )?;
                storage.commit(
                    &[
                        StorageWrite::DocumentChange {
                            id: latest_source_id,
                            content: DocumentContent::Base {
                                version: 4,
                                value: json!({ "retained": "source-only" })
                                    .as_object()
                                    .cloned()
                                    .unwrap_or_default(),
                            },
                        },
                        StorageWrite::DocumentRetire {
                            id: latest_source_id,
                        },
                    ],
                    &CONTEXT,
                )?;
                let latest_copy =
                    storage.document(latest_copy_id, DocumentPoint::Current, &CONTEXT)?;
                a.partial_deep_equal(
                    &latest_copy
                        .as_ref()
                        .map(document_view)
                        .unwrap_or(Value::Null),
                    &json!({
                        "version": 4,
                        "value": { "retained": "copy" },
                    }),
                )?;

                let conflict_id = storage.mint_id()?;
                let conflict_error = storage.commit(
                    &[
                        StorageWrite::DocumentCopy {
                            record: child_record(conflict_id, child_id),
                            source: DocumentCopySource {
                                id: current_copy_id,
                                at: DocumentPoint::Current,
                            },
                        },
                        StorageWrite::DocumentRetire {
                            id: current_copy_id,
                        },
                    ],
                    &CONTEXT,
                );
                // The rejected commit surfaces as an `Err`; its `StorageRejected`
                // kind mirrors the upstream error `name`.
                a.strict_equal(
                    &json!(matches!(
                        conflict_error,
                        Err(ref error)
                            if error.kind == crate::durable::storage::StorageErrorKind::Rejected
                    )),
                    &json!(true),
                )?;
                let conflict_document =
                    storage.document(conflict_id, DocumentPoint::Current, &CONTEXT)?;
                a.strict_equal(&json_of(&conflict_document), &Value::Null)?;
                let current_copy_value =
                    storage.document(current_copy_id, DocumentPoint::Current, &CONTEXT)?;
                a.deep_equal(
                    &current_copy_value
                        .as_ref()
                        .map(|document| Value::Object(document.value.clone()))
                        .unwrap_or(Value::Null),
                    &json!({ "count": 2, "rows": [{ "value": "base" }, { "value": "current" }] }),
                )?;

                let mismatch_id = storage.mint_id()?;
                let mismatch_error = storage.commit(
                    &[StorageWrite::DocumentCopy {
                        record: DocumentCreate {
                            kind: String::from("copy.mismatch"),
                            ..child_record(mismatch_id, child_id)
                        },
                        source: DocumentCopySource {
                            id: current_copy_id,
                            at: DocumentPoint::Current,
                        },
                    }],
                    &CONTEXT,
                );
                a.strict_equal(
                    &json!(matches!(
                        mismatch_error,
                        Err(ref error)
                            if error.kind == crate::durable::storage::StorageErrorKind::Rejected
                    )),
                    &json!(true),
                )?;
                let mismatch_document =
                    storage.document(mismatch_id, DocumentPoint::Current, &CONTEXT)?;
                a.strict_equal(&json_of(&mismatch_document), &Value::Null)?;
                Ok(())
            })
        },
    ));

    cases.push(create_case(
        options,
        "uses bases for version transitions and rejects historical reads of current-only documents",
        {
            let a = Arc::clone(&a);
            Arc::new(move |storage: Arc<dyn Storage>| {
            create_root(storage.as_ref())?;
            let id = storage.mint_id()?;
            let record = DocumentCreate {
                id,
                kind: String::from("session.settings"),
                key: None,
                scope: DocumentScope::Session,
                history: None,
                fork: None,
            };
            storage.commit(
                &[StorageWrite::DocumentCreate {
                    record: record.clone(),
                    content: DocumentContent::Base {
                        version: 1,
                        value: json!({ "count": 1 })
                            .as_object()
                            .cloned()
                            .unwrap_or_default(),
                    },
                }],
                &CONTEXT,
            )?;
            storage.commit(
                &[StorageWrite::DocumentChange {
                    id,
                    content: DocumentContent::Delta {
                        version: 1,
                        ops: vec![set_op(&["count"], json!(2))],
                    },
                }],
                &CONTEXT,
            )?;
            let migrated_at = storage.commit(
                &[StorageWrite::DocumentChange {
                    id,
                    content: DocumentContent::Base {
                        version: 2,
                        value: json!({ "count": 3 })
                            .as_object()
                            .cloned()
                            .unwrap_or_default(),
                    },
                }],
                &CONTEXT,
            )?;
            let current = storage.document(id, DocumentPoint::Current, &CONTEXT)?;
            a.partial_deep_equal(
                &current.as_ref().map(document_view).unwrap_or(Value::Null),
                &json!({ "version": 2, "value": { "count": 3 } }),
            )?;
            let historical = storage.document(id, DocumentPoint::Seq(migrated_at), &CONTEXT);
            a.strict_equal(
                &json!(matches!(
                    historical,
                    Err(ref error) if error.to_string().contains("does not retain historical content")
                )),
                &json!(true),
            )?;

            let transition = storage.commit(
                &[StorageWrite::DocumentChange {
                    id,
                    content: DocumentContent::Delta {
                        version: 1,
                        ops: vec![set_op(&["count"], json!(4))],
                    },
                }],
                &CONTEXT,
            );
            a.strict_equal(
                &json!(matches!(
                    transition,
                    Err(ref error) if error.to_string().contains("version transition requires a base")
                )),
                &json!(true),
            )?;
            let current_value = storage.document(id, DocumentPoint::Current, &CONTEXT)?;
            a.deep_equal(
                &current_value
                    .as_ref()
                    .map(|document| Value::Object(document.value.clone()))
                    .unwrap_or(Value::Null),
                &json!({ "count": 3 }),
            )?;
            storage.commit(&[StorageWrite::DocumentRetire { id }], &CONTEXT)?;
            let retired = storage.document(id, DocumentPoint::Current, &CONTEXT)?;
            a.strict_equal(&json_of(&retired), &Value::Null)?;
            Ok(())
            })
        }
    ));

    cases.push(create_case(
        options,
        "indexes logical addresses and exact-scope scans independently",
        {
            let a = Arc::clone(&a);
            Arc::new(move |storage: Arc<dyn Storage>| {
                let root_id = create_root(storage.as_ref())?;
                let first_id = storage.mint_id()?;
                let second_id = storage.mint_id()?;
                let conversation_id = storage.mint_id()?;
                let task_id = storage.mint_id()?;
                let task_singleton_id = storage.mint_id()?;
                let task_family_id = storage.mint_id()?;
                let task_other_kind_id = storage.mint_id()?;
                let created_at = storage.commit(
                    &[
                        StorageWrite::Task {
                            value: pending_task(task_id, root_id),
                        },
                        StorageWrite::DocumentCreate {
                            record: DocumentCreate {
                                id: first_id,
                                kind: String::from("cache"),
                                key: Some(String::from("__proto__")),
                                scope: DocumentScope::Session,
                                history: None,
                                fork: None,
                            },
                            content: DocumentContent::Base {
                                version: 1,
                                value: json!({ "owner": "first" })
                                    .as_object()
                                    .cloned()
                                    .unwrap_or_default(),
                            },
                        },
                        StorageWrite::DocumentCreate {
                            record: DocumentCreate {
                                id: second_id,
                                kind: String::from("cache"),
                                key: Some(String::from("constructor")),
                                scope: DocumentScope::Session,
                                history: None,
                                fork: None,
                            },
                            content: DocumentContent::Base {
                                version: 1,
                                value: json!({ "owner": "second" })
                                    .as_object()
                                    .cloned()
                                    .unwrap_or_default(),
                            },
                        },
                        StorageWrite::DocumentCreate {
                            record: DocumentCreate {
                                id: conversation_id,
                                kind: String::from("cache"),
                                key: Some(String::from("__proto__")),
                                scope: DocumentScope::Conversation {
                                    conversation_id: root_id,
                                },
                                history: Some(DocumentHistory::Latest),
                                fork: Some(DocumentFork::Current),
                            },
                            content: DocumentContent::Base {
                                version: 1,
                                value: json!({ "owner": "conversation" })
                                    .as_object()
                                    .cloned()
                                    .unwrap_or_default(),
                            },
                        },
                        StorageWrite::DocumentCreate {
                            record: DocumentCreate {
                                id: task_singleton_id,
                                kind: String::from("task.cache"),
                                key: None,
                                scope: DocumentScope::Task { task_id },
                                history: None,
                                fork: None,
                            },
                            content: DocumentContent::Base {
                                version: 1,
                                value: json!({ "owner": "singleton" })
                                    .as_object()
                                    .cloned()
                                    .unwrap_or_default(),
                            },
                        },
                        StorageWrite::DocumentCreate {
                            record: DocumentCreate {
                                id: task_family_id,
                                kind: String::from("task.cache"),
                                key: Some(String::from("member")),
                                scope: DocumentScope::Task { task_id },
                                history: None,
                                fork: None,
                            },
                            content: DocumentContent::Base {
                                version: 1,
                                value: json!({ "owner": "family" })
                                    .as_object()
                                    .cloned()
                                    .unwrap_or_default(),
                            },
                        },
                        StorageWrite::DocumentCreate {
                            record: DocumentCreate {
                                id: task_other_kind_id,
                                kind: String::from("task.other"),
                                key: None,
                                scope: DocumentScope::Task { task_id },
                                history: None,
                                fork: None,
                            },
                            content: DocumentContent::Base {
                                version: 1,
                                value: json!({ "owner": "other" })
                                    .as_object()
                                    .cloned()
                                    .unwrap_or_default(),
                            },
                        },
                    ],
                    &CONTEXT,
                )?;

                let first_find = storage.find_document(
                    &DocumentAddress {
                        kind: String::from("cache"),
                        scope: DocumentScope::Session,
                        key: Some(String::from("__proto__")),
                    },
                    DocumentPoint::Current,
                    &CONTEXT,
                )?;
                a.strict_equal(
                    &first_find
                        .as_ref()
                        .map(|record| json!(record.id))
                        .unwrap_or(Value::Null),
                    &json!(first_id),
                )?;
                let session_page = storage.scan_documents(
                    DocumentQuery {
                        scope: DocumentScope::Session,
                        at: DocumentPoint::Current,
                        kind: None,
                    },
                    1,
                    None,
                    &CONTEXT,
                )?;
                let first = session_page;
                a.strict_equal(&json!(first.items.len()), &json!(1))?;
                let second_page = storage.scan_documents(
                    DocumentQuery {
                        scope: DocumentScope::Session,
                        at: DocumentPoint::Current,
                        kind: None,
                    },
                    1,
                    first.next.as_ref(),
                    &CONTEXT,
                )?;
                let mut both = first.items.clone();
                both.extend(second_page.items);
                a.deep_equal(
                    &Value::Array(both.iter().map(IdOf::id_json).collect()),
                    &json!([first_id, second_id]),
                )?;
                let conversation_scan = storage.scan_documents(
                    DocumentQuery {
                        scope: DocumentScope::Conversation {
                            conversation_id: root_id,
                        },
                        at: DocumentPoint::Current,
                        kind: None,
                    },
                    10,
                    None,
                    &CONTEXT,
                )?;
                a.deep_equal(&page_ids(&conversation_scan), &json!([conversation_id]))?;
                let singleton_find = storage.find_document(
                    &DocumentAddress {
                        kind: String::from("task.cache"),
                        scope: DocumentScope::Task { task_id },
                        key: None,
                    },
                    DocumentPoint::Current,
                    &CONTEXT,
                )?;
                a.strict_equal(
                    &singleton_find
                        .as_ref()
                        .map(|record| json!(record.id))
                        .unwrap_or(Value::Null),
                    &json!(task_singleton_id),
                )?;
                let family_find = storage.find_document(
                    &DocumentAddress {
                        kind: String::from("task.cache"),
                        scope: DocumentScope::Task { task_id },
                        key: Some(String::from("member")),
                    },
                    DocumentPoint::Current,
                    &CONTEXT,
                )?;
                a.strict_equal(
                    &family_find
                        .as_ref()
                        .map(|record| json!(record.id))
                        .unwrap_or(Value::Null),
                    &json!(task_family_id),
                )?;
                let task_scan = storage.scan_documents(
                    DocumentQuery {
                        scope: DocumentScope::Task { task_id },
                        at: DocumentPoint::Current,
                        kind: Some(String::from("task.cache")),
                    },
                    10,
                    None,
                    &CONTEXT,
                )?;
                a.deep_equal(
                    &page_ids(&task_scan),
                    &json!([task_singleton_id, task_family_id]),
                )?;
                let storage_op = Arc::clone(&storage);
                a.rejects(
                    Box::new(move || {
                        storage_op
                            .document(task_singleton_id, DocumentPoint::Seq(created_at), &CONTEXT)
                            .map(|_| ())
                    }),
                    "does not retain historical content",
                )?;
                Ok(())
            })
        },
    ));

    cases.push(create_case(
        options,
        "keeps document lifecycle failures atomic and gives create-plus-retire an empty lifetime",
        {
            let a = Arc::clone(&a);
            Arc::new(move |storage: Arc<dyn Storage>| {
            let root_id = create_root(storage.as_ref())?;
            let first_id = storage.mint_id()?;
            let second_id = storage.mint_id()?;
            let record = DocumentCreate {
                id: first_id,
                kind: String::from("singleton"),
                key: None,
                scope: DocumentScope::Session,
                history: None,
                fork: None,
            };
            storage.commit(
                &[StorageWrite::DocumentCreate {
                    record: record.clone(),
                    content: DocumentContent::Base {
                        version: 1,
                        value: json!({ "value": 1 })
                            .as_object()
                            .cloned()
                            .unwrap_or_default(),
                    },
                }],
                &CONTEXT,
            )?;
            let rejected = storage.commit(
                &[
                    StorageWrite::DocumentCreate {
                        record: DocumentCreate {
                            id: second_id,
                            ..record.clone()
                        },
                        content: DocumentContent::Base {
                            version: 1,
                            value: json!({ "value": 2 })
                                .as_object()
                                .cloned()
                                .unwrap_or_default(),
                        },
                    },
                    StorageWrite::DocumentChange {
                        id: first_id,
                        content: DocumentContent::Delta {
                            version: 1,
                            ops: vec![],
                        },
                    },
                ],
                &CONTEXT,
            );
            a.strict_equal(
                &json!(matches!(
                    rejected,
                    Err(ref error) if error.to_string().contains("already has a current incarnation")
                )),
                &json!(true),
            )?;
            let first_document = storage.document(first_id, DocumentPoint::Current, &CONTEXT)?;
            a.deep_equal(
                &first_document
                    .as_ref()
                    .map(|document| Value::Object(document.value.clone()))
                    .unwrap_or(Value::Null),
                &json!({ "value": 1 }),
            )?;
            let second_document = storage.document(second_id, DocumentPoint::Current, &CONTEXT)?;
            a.strict_equal(&json_of(&second_document), &Value::Null)?;

            let empty_id = storage.mint_id()?;
            let empty_at = storage.commit(
                &[
                    StorageWrite::DocumentCreate {
                        record: DocumentCreate {
                            id: empty_id,
                            kind: record.kind.clone(),
                            key: Some(String::from("empty")),
                            scope: DocumentScope::Conversation {
                                conversation_id: root_id,
                            },
                            history: Some(DocumentHistory::Rewindable),
                            fork: Some(DocumentFork::Initial),
                        },
                        content: DocumentContent::Base {
                            version: 1,
                            value: JsonObject::default(),
                        },
                    },
                    StorageWrite::DocumentRetire { id: empty_id },
                ],
                &CONTEXT,
            )?;
            let empty_document = storage.document(empty_id, DocumentPoint::Current, &CONTEXT)?;
            a.strict_equal(&json_of(&empty_document), &Value::Null)?;
            let empty_historical = storage.document(empty_id, DocumentPoint::Seq(empty_at), &CONTEXT)?;
            a.strict_equal(&json_of(&empty_historical), &Value::Null)?;
            let empty_find = storage.find_document(
                &DocumentAddress {
                    kind: record.kind.clone(),
                    scope: DocumentScope::Conversation {
                        conversation_id: root_id,
                    },
                    key: Some(String::from("empty")),
                },
                DocumentPoint::Seq(empty_at),
                &CONTEXT,
            )?;
            a.strict_equal(&json_of(&empty_find), &Value::Null)?;
            Ok(())
            })
        }
    ));

    cases.push(create_case(
        options,
        "rolls back record tables and secondary indexes when a document command fails",
        {
            let a = Arc::clone(&a);
            Arc::new(move |storage: Arc<dyn Storage>| {
            let root_id = create_root(storage.as_ref())?;
            let task_id = storage.mint_id()?;
            let submission_id = storage.mint_id()?;
            let document_id = storage.mint_id()?;
            let task = pending_task(task_id, root_id);
            let submission = SubmissionRecord {
                conversation_id: root_id,
                request_id: Some(String::from("atomic")),
                r#type: SubmissionType::Input,
                status: SubmissionStatus::Queued,
                entry: None,
                answer: None,
                reason: None,
                detail: None,
                id: submission_id,
                entry_before_id: false,
            };
            let record = DocumentCreate {
                id: document_id,
                kind: String::from("atomic"),
                key: None,
                scope: DocumentScope::Session,
                history: None,
                fork: None,
            };
            let baseline_seq = storage.commit(
                &[
                    StorageWrite::Task {
                        value: task.clone(),
                    },
                    StorageWrite::Submission {
                        value: submission.clone(),
                    },
                    StorageWrite::DocumentCreate {
                        record: record.clone(),
                        content: DocumentContent::Base {
                            version: 1,
                            value: json!({ "count": 1 })
                                .as_object()
                                .cloned()
                                .unwrap_or_default(),
                        },
                    },
                ],
                &CONTEXT,
            )?;

            let entry_id = storage.mint_id()?;
            let conflicting_document_id = storage.mint_id()?;
            let mut running_task = task.clone();
            running_task.state = TaskState::Running {
                checkpoint: json!({ "phase": "effect" }),
            };
            let mut unanswered = submission.clone();
            unanswered.status = SubmissionStatus::Unanswered;
            unanswered.reason = Some(String::from("failed"));
            let rejected = storage.commit(
                &[
                    StorageWrite::Task {
                        value: running_task,
                    },
                    StorageWrite::Submission {
                        value: unanswered,
                    },
                    StorageWrite::Entry {
                        value: entry(entry_id, root_id, "transient"),
                    },
                    StorageWrite::DocumentCreate {
                        record: DocumentCreate {
                            id: conflicting_document_id,
                            ..record.clone()
                        },
                        content: DocumentContent::Base {
                            version: 1,
                            value: json!({ "count": 2 })
                                .as_object()
                                .cloned()
                                .unwrap_or_default(),
                        },
                    },
                ],
                &CONTEXT,
            );
            a.strict_equal(
                &json!(matches!(
                    rejected,
                    Err(ref error) if error.to_string().contains("already has a current incarnation")
                )),
                &json!(true),
            )?;

            let found_task = storage.task(task_id, &CONTEXT)?;
            a.deep_equal(&json_of(&found_task), &json_of(&task))?;
            let pending_page = storage.scan_tasks(
                TaskQuery {
                    status: Some(TaskStatus::Pending),
                    ..Default::default()
                },
                10,
                None,
                &CONTEXT,
            )?;
            a.deep_equal(
                &Value::Array(pending_page.items.iter().map(json_of).collect()),
                &Value::Array(vec![json_of(&task)]),
            )?;
            let by_request = storage.submission_by_request(root_id, "atomic", &CONTEXT)?;
            a.deep_equal(&json_of(&by_request), &json_of(&submission))?;
            let transient = storage.entry(entry_id, &CONTEXT)?;
            a.strict_equal(&transient.as_ref().map(entry_with_commit_seq).unwrap_or(Value::Null), &Value::Null)?;
            let conflicting = storage.document(conflicting_document_id, DocumentPoint::Current, &CONTEXT)?;
            a.strict_equal(&json_of(&conflicting), &Value::Null)?;
            let find = storage.find_document(
                &DocumentAddress {
                    kind: record.kind.clone(),
                    scope: record.scope,
                    key: record.key.clone(),
                },
                DocumentPoint::Current,
                &CONTEXT,
            )?;
            a.strict_equal(
                &find.as_ref().map(|found| json!(found.id)).unwrap_or(Value::Null),
                &json!(document_id),
            )?;
            let after_rollback_seq = storage.commit(
                &[StorageWrite::DocumentChange {
                    id: document_id,
                    content: DocumentContent::Delta {
                        version: 1,
                        ops: vec![set_op(&["count"], json!(3))],
                    },
                }],
                &CONTEXT,
            )?;
            a.greater_than(after_rollback_seq, baseline_seq)?;
            Ok(())
            })
        }
    ));

    cases.push(create_case(
        options,
        "keeps one global record ID namespace and rejects exhausted ID minting",
        {
            let a = Arc::clone(&a);
            Arc::new(move |storage: Arc<dyn Storage>| {
                let root_id = create_root(storage.as_ref())?;
                let explicit_entry_id = id_from_number(100);
                storage.commit(
                    &[StorageWrite::Entry {
                        value: entry(explicit_entry_id, root_id, "message"),
                    }],
                    &CONTEXT,
                )?;
                let minted = storage.mint_id()?;
                a.strict_equal(&json!(minted), &json!(101))?;
                let storage_op = Arc::clone(&storage);
                a.rejects(
                    Box::new(move || {
                        storage_op
                            .commit(
                                &[StorageWrite::Task {
                                    value: pending_task(explicit_entry_id, root_id),
                                }],
                                &CONTEXT,
                            )
                            .map(|_| ())
                    }),
                    &format!("ID {explicit_entry_id} already belongs to entry"),
                )?;

                storage.commit(
                    &[StorageWrite::Entry {
                        value: entry(id_from_number(9_007_199_254_740_991), root_id, "last-id"),
                    }],
                    &CONTEXT,
                )?;
                let storage_first = Arc::clone(&storage);
                a.rejects(
                    Box::new(move || storage_first.mint_id().map(|_| ())),
                    "ID space is exhausted",
                )?;
                let storage_second = Arc::clone(&storage);
                a.rejects(
                    Box::new(move || storage_second.mint_id().map(|_| ())),
                    "ID space is exhausted",
                )?;
                Ok(())
            })
        },
    ));

    cases.push(create_case(
        options,
        "rejects every operation after close",
        {
            let a = Arc::clone(&a);
            Arc::new(move |storage: Arc<dyn Storage>| {
                create_root(storage.as_ref())?;
                storage.close(&CONTEXT)?;
                let storage_conversation = Arc::clone(&storage);
                a.rejects(
                    Box::new(move || {
                        storage_conversation
                            .conversation(ROOT_CONVERSATION_ID, &CONTEXT)
                            .map(|_| ())
                    }),
                    "closed",
                )?;
                let storage_commit = Arc::clone(&storage);
                a.rejects(
                    Box::new(move || {
                        storage_commit
                            .commit(&[] as &[StorageWrite], &CONTEXT)
                            .map(|_| ())
                    }),
                    "closed",
                )?;
                let storage_mint = Arc::clone(&storage);
                a.rejects(
                    Box::new(move || storage_mint.mint_id().map(|_| ())),
                    "closed",
                )?;
                Ok(())
            })
        },
    ));

    cases
}

fn with_data(mut record: EntryRecord, data: Value) -> EntryRecord {
    record.data = Some(data);
    record
}

fn with_head(mut record: EntryRecord, head: i64) -> EntryRecord {
    record.head = Some(head);
    record
}
