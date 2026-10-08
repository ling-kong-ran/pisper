//! Port of `src/testing/storage-benchmark.ts`: deterministic representative
//! datasets seeded through only the public `Storage` contract, plus the
//! read/write benchmark tables with their expected outcomes.

use serde_json::{json, Value};

use crate::agent_core::chord_support::context::Context;
use crate::chord::delta::Op;
use crate::durable::ids::ROOT_CONVERSATION_ID;
use crate::durable::storage::Storage;
use crate::durable::types::{
    ConversationRecord, DocumentContent, DocumentCreate, DocumentFork, DocumentHistory,
    DocumentPoint, DocumentScope, EntryRecord, JsonObject, StorageWrite, SubmissionRecord,
    SubmissionStatus, SubmissionType, TaskRecord, TaskState,
};

/// `BACKGROUND_CONTEXT` (`@earendil-works/chord/context`).
pub fn background_context() -> Context {
    Context::background()
}

/// `StorageBenchmarkScale` (`testing/storage-benchmark.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageBenchmarkScale {
    pub name: &'static str,
    pub entry_count: usize,
    pub task_count: usize,
    pub document_count: usize,
}

/// `STORAGE_MEMORY_SCALES` (`testing/storage-benchmark.ts`).
pub const STORAGE_MEMORY_SCALES: [StorageBenchmarkScale; 2] = [
    StorageBenchmarkScale {
        name: "1k",
        entry_count: 1_000,
        task_count: 200,
        document_count: 200,
    },
    StorageBenchmarkScale {
        name: "10k",
        entry_count: 10_000,
        task_count: 2_000,
        document_count: 2_000,
    },
];

/// `TIMING_SCALE` (`testing/storage-benchmark.ts`).
pub const TIMING_SCALE: StorageBenchmarkScale = StorageBenchmarkScale {
    name: "timing",
    entry_count: 1_000,
    task_count: 300,
    document_count: 300,
};

const REPLAY_TAILS: [usize; 4] = [0, 16, 128, 1_024];
const HISTORY_SEGMENT_LENGTH: usize = 128;
const FORK_DEPTH: usize = 8;
const ENTRIES_PER_FORK: usize = 32;
const BATCH_SIZE: usize = 100;

/// `storageBenchmarkPrimaryRecordCount(scale)`
/// (`testing/storage-benchmark.ts`).
pub fn storage_benchmark_primary_record_count(scale: &StorageBenchmarkScale) -> usize {
    1 + scale.entry_count
        + scale.task_count
        + scale.document_count
        + REPLAY_TAILS.len()
        + 1
        + FORK_DEPTH * (1 + ENTRIES_PER_FORK)
}

/// `StorageBenchmarkDataset` (`testing/storage-benchmark.ts`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageBenchmarkDataset {
    pub first_entry_id: i64,
    pub filtered_task_count: usize,
    pub exact_document_id: i64,
    pub exact_document_key: String,
    /// Keyed by replay tail (`0`, `16`, `128`, `1024`).
    pub replay_document_ids: [(usize, i64); 4],
    pub historical_document_id: i64,
    pub ancient_at: i64,
    pub recent_at: i64,
    pub deepest_conversation_id: i64,
    pub ancestor_head_entry_id: i64,
}

impl StorageBenchmarkDataset {
    /// `dataset.replayDocumentIds[tail]` (borrowed for reads only).
    pub fn replay_document_id(&self, tail: usize) -> i64 {
        self.replay_document_ids
            .iter()
            .find(|(key, _)| *key == tail)
            .map(|(_, id)| *id)
            .expect("known replay tail")
    }

    /// The dataset's JSON wire form (key order per the upstream literal).
    pub fn to_json(&self) -> JsonObject {
        let mut map = JsonObject::new();
        map.insert(String::from("firstEntryId"), json!(self.first_entry_id));
        map.insert(
            String::from("filteredTaskCount"),
            json!(self.filtered_task_count),
        );
        map.insert(
            String::from("exactDocumentId"),
            json!(self.exact_document_id),
        );
        map.insert(
            String::from("exactDocumentKey"),
            json!(self.exact_document_key),
        );
        let mut replay = JsonObject::new();
        for (tail, id) in self.replay_document_ids {
            replay.insert(json!(tail).to_string(), json!(id));
        }
        map.insert(String::from("replayDocumentIds"), Value::Object(replay));
        map.insert(
            String::from("historicalDocumentId"),
            json!(self.historical_document_id),
        );
        map.insert(String::from("ancientAt"), json!(self.ancient_at));
        map.insert(String::from("recentAt"), json!(self.recent_at));
        map.insert(
            String::from("deepestConversationId"),
            json!(self.deepest_conversation_id),
        );
        map.insert(
            String::from("ancestorHeadEntryId"),
            json!(self.ancestor_head_entry_id),
        );
        map
    }
}

/// `task(id, index)` (`testing/storage-benchmark.ts`).
fn task(id: i64, index: usize) -> TaskRecord {
    let statuses = ["pending", "running", "terminal"];
    let status = statuses[index % statuses.len()];
    if status == "terminal" {
        TaskRecord {
            id,
            conversation_id: ROOT_CONVERSATION_ID,
            kind: String::from(if index.is_multiple_of(4) {
                "benchmark.filtered"
            } else {
                "benchmark.other"
            }),
            version: 1,
            input: json!({ "index": index }),
            owner: None,
            background: index.is_multiple_of(5),
            abort_requested: index.is_multiple_of(7),
            state: TaskState::Terminal {
                outcome: crate::durable::types::TaskOutcome::Completed {
                    result: json!({ "index": index }),
                },
            },
            memos: None,
        }
    } else {
        TaskRecord {
            id,
            conversation_id: ROOT_CONVERSATION_ID,
            kind: String::from(if index.is_multiple_of(4) {
                "benchmark.filtered"
            } else {
                "benchmark.other"
            }),
            version: 1,
            input: json!({ "index": index }),
            owner: None,
            background: index.is_multiple_of(5),
            abort_requested: index.is_multiple_of(7),
            state: TaskState::Pending {
                checkpoint: json!({ "index": index, "payload": "x".repeat(64) }),
            },
            memos: None,
        }
    }
}

fn set_op(path: &[&str], value: serde_json::Value) -> Op {
    Op::Set {
        path: path
            .iter()
            .map(|segment| crate::chord::delta::Seg::Key((*segment).to_string()))
            .collect(),
        value,
    }
}

/// `seedStorageBenchmark(storage, scale)` (`testing/storage-benchmark.ts`):
/// seed deterministic representative data through only the public `Storage`
/// contract.
pub fn seed_storage_benchmark(
    storage: &dyn Storage,
    scale: &StorageBenchmarkScale,
) -> Result<StorageBenchmarkDataset, crate::durable::storage::StorageError> {
    let context = background_context();
    storage.commit(
        &[StorageWrite::Conversation {
            value: ConversationRecord {
                id: ROOT_CONVERSATION_ID,
                parent: None,
                owner: None,
            },
        }],
        &context,
    )?;

    let mut first_entry_id = None;
    let mut start = 0;
    while start < scale.entry_count {
        let mut writes = Vec::new();
        for index in start..scale.entry_count.min(start + BATCH_SIZE) {
            let id = storage.mint_id()?;
            if index == 0 {
                first_entry_id = Some(id);
            }
            writes.push(StorageWrite::Entry {
                value: EntryRecord {
                    kind: String::from("benchmark.entry"),
                    model: None,
                    data: Some(json!({ "index": index, "text": format!("entry-{index}-{}", "x".repeat(96)) })),
                    edits: None,
                    id,
                    conversation_id: ROOT_CONVERSATION_ID,
                    head: (index == 0).then_some(id),
                    by_task_id: None,
                },
            });
        }
        storage.commit(&writes, &context)?;
        start += BATCH_SIZE;
    }

    let mut start = 0;
    while start < scale.task_count {
        let mut writes = Vec::new();
        for index in start..scale.task_count.min(start + BATCH_SIZE) {
            writes.push(StorageWrite::Task {
                value: task(storage.mint_id()?, index),
            });
        }
        storage.commit(&writes, &context)?;
        start += BATCH_SIZE;
    }

    let mut exact_document_id = None;
    let mut start = 0;
    while start < scale.document_count {
        let mut writes = Vec::new();
        for index in start..scale.document_count.min(start + BATCH_SIZE) {
            let id = storage.mint_id()?;
            exact_document_id = Some(id);
            writes.push(StorageWrite::DocumentCreate {
                record: DocumentCreate {
                    id,
                    kind: String::from("benchmark.family"),
                    key: Some(format!("key-{index}")),
                    scope: DocumentScope::Session,
                    history: None,
                    fork: None,
                },
                content: DocumentContent::Base {
                    version: 1,
                    value: json!({ "index": index, "text": "x".repeat(128) })
                        .as_object()
                        .cloned()
                        .unwrap_or_default(),
                },
            });
        }
        storage.commit(&writes, &context)?;
        start += BATCH_SIZE;
    }

    let mut replay_entries = Vec::new();
    for tail in REPLAY_TAILS {
        let id = storage.mint_id()?;
        replay_entries.push((tail, id));
    }
    storage.commit(
        &replay_entries
            .iter()
            .map(|(tail, id)| StorageWrite::DocumentCreate {
                record: DocumentCreate {
                    id: *id,
                    kind: String::from("benchmark.replay"),
                    key: Some(tail.to_string()),
                    scope: DocumentScope::Conversation {
                        conversation_id: ROOT_CONVERSATION_ID,
                    },
                    history: Some(DocumentHistory::Rewindable),
                    fork: Some(DocumentFork::AsOf),
                },
                content: DocumentContent::Base {
                    version: 1,
                    value: json!({ "count": 0, "text": "x".repeat(64) })
                        .as_object()
                        .cloned()
                        .unwrap_or_default(),
                },
            })
            .collect::<Vec<_>>(),
        &context,
    )?;
    for count in 1..=*REPLAY_TAILS.last().expect("non-empty") {
        storage.commit(
            &replay_entries
                .iter()
                .filter(|(tail, _)| count <= *tail)
                .map(|(_, id)| StorageWrite::DocumentChange {
                    id: *id,
                    content: DocumentContent::Delta {
                        version: 1,
                        ops: vec![set_op(&["count"], json!(count))],
                    },
                })
                .collect::<Vec<_>>(),
            &context,
        )?;
    }

    let historical_document_id = storage.mint_id()?;
    storage.commit(
        &[StorageWrite::DocumentCreate {
            record: DocumentCreate {
                id: historical_document_id,
                kind: String::from("benchmark.history"),
                key: None,
                scope: DocumentScope::Conversation {
                    conversation_id: ROOT_CONVERSATION_ID,
                },
                history: Some(DocumentHistory::Rewindable),
                fork: Some(DocumentFork::AsOf),
            },
            content: DocumentContent::Base {
                version: 1,
                value: json!({ "count": 0 })
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
            },
        }],
        &context,
    )?;
    let mut ancient_at = None;
    for count in 1..=HISTORY_SEGMENT_LENGTH {
        ancient_at = Some(storage.commit(
            &[StorageWrite::DocumentChange {
                id: historical_document_id,
                content: DocumentContent::Delta {
                    version: 1,
                    ops: vec![set_op(&["count"], json!(count))],
                },
            }],
            &context,
        )?);
    }
    storage.commit(
        &[StorageWrite::DocumentChange {
            id: historical_document_id,
            content: DocumentContent::Base {
                version: 1,
                value: json!({ "count": HISTORY_SEGMENT_LENGTH })
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
            },
        }],
        &context,
    )?;
    let Some(ancient_at) = ancient_at else {
        return Err(crate::durable::storage::StorageError::generic(
            "Benchmark history seed produced no commits",
        ));
    };
    let mut recent_at = ancient_at;
    for count in HISTORY_SEGMENT_LENGTH + 1..=HISTORY_SEGMENT_LENGTH * 2 {
        recent_at = storage.commit(
            &[StorageWrite::DocumentChange {
                id: historical_document_id,
                content: DocumentContent::Delta {
                    version: 1,
                    ops: vec![set_op(&["count"], json!(count))],
                },
            }],
            &context,
        )?;
    }

    let Some(first_entry_id) = first_entry_id else {
        return Err(crate::durable::storage::StorageError::generic(
            "Benchmark scale must create entries",
        ));
    };
    let mut parent_conversation_id = ROOT_CONVERSATION_ID;
    let mut parent_at = first_entry_id;
    let mut deepest_conversation_id = ROOT_CONVERSATION_ID;
    for depth in 0..FORK_DEPTH {
        let conversation_id = storage.mint_id()?;
        storage.commit(
            &[StorageWrite::Conversation {
                value: ConversationRecord {
                    id: conversation_id,
                    parent: Some(crate::durable::types::ConversationParent {
                        conversation_id: parent_conversation_id,
                        at: parent_at,
                    }),
                    owner: None,
                },
            }],
            &context,
        )?;
        let ids = (0..ENTRIES_PER_FORK)
            .map(|_| storage.mint_id())
            .collect::<Result<Vec<_>, _>>()?;
        storage.commit(
            &ids.iter()
                .enumerate()
                .map(|(index, id)| StorageWrite::Entry {
                    value: EntryRecord {
                        kind: String::from("benchmark.fork"),
                        model: None,
                        data: Some(json!({ "depth": depth, "index": index })),
                        edits: None,
                        id: *id,
                        conversation_id,
                        head: None,
                        by_task_id: None,
                    },
                })
                .collect::<Vec<_>>(),
            &context,
        )?;
        parent_conversation_id = conversation_id;
        parent_at = *ids.last().expect("non-empty");
        deepest_conversation_id = conversation_id;
    }

    let Some(exact_document_id) = exact_document_id else {
        return Err(crate::durable::storage::StorageError::generic(
            "Benchmark scale must create documents",
        ));
    };
    Ok(StorageBenchmarkDataset {
        first_entry_id,
        filtered_task_count: 50.min(scale.task_count.div_ceil(60)),
        exact_document_id,
        exact_document_key: format!("key-{}", scale.document_count - 1),
        replay_document_ids: replay_entries
            .iter()
            .map(|(tail, id)| (*tail, *id))
            .collect::<Vec<_>>()
            .try_into()
            .expect("four replay tails"),
        historical_document_id,
        ancient_at,
        recent_at,
        deepest_conversation_id,
        ancestor_head_entry_id: first_entry_id,
    })
}

/// `StorageReadBenchmark` (`testing/storage-benchmark.ts`). The tail-table
/// rows close over their replay tail, so the steps are boxed closures.
pub struct StorageReadBenchmark {
    pub name: &'static str,
    pub run: ReadBenchmarkRun,
    pub expected: Box<dyn Fn(&StorageBenchmarkDataset) -> i64>,
}

/// `STORAGE_READ_BENCHMARKS` (`testing/storage-benchmark.ts`).
pub fn storage_read_benchmarks() -> Vec<StorageReadBenchmark> {
    let mut benchmarks = vec![
        StorageReadBenchmark {
            name: "exact entry lookup",
            run: Box::new(|storage: &dyn Storage, dataset: &StorageBenchmarkDataset| {
                Ok(storage
                    .entry(dataset.first_entry_id, &background_context())?
                    .map(|found| found.entry.id)
                    .unwrap_or(-1))
            }),
            expected: Box::new(|dataset: &StorageBenchmarkDataset| dataset.first_entry_id),
        },
        StorageReadBenchmark {
            name: "entry page scan (100)",
            run: Box::new(
                |storage: &dyn Storage, _dataset: &StorageBenchmarkDataset| {
                    Ok(storage
                        .scan_entries(
                            crate::durable::types::EntryQuery {
                                conversation_id: ROOT_CONVERSATION_ID,
                                min_entry_id: None,
                                max_entry_id: None,
                            },
                            100,
                            None,
                            &background_context(),
                        )?
                        .items
                        .len() as i64)
                },
            ),
            expected: Box::new(|_dataset: &StorageBenchmarkDataset| 100),
        },
        StorageReadBenchmark {
            name: "filtered task scan (50)",
            run: Box::new(
                |storage: &dyn Storage, _dataset: &StorageBenchmarkDataset| {
                    Ok(storage
                        .scan_tasks(
                            crate::durable::types::TaskQuery {
                                kind: Some(String::from("benchmark.filtered")),
                                status: Some(crate::durable::types::TaskStatus::Pending),
                                background: Some(true),
                                ..Default::default()
                            },
                            50,
                            None,
                            &background_context(),
                        )?
                        .items
                        .len() as i64)
                },
            ),
            expected: Box::new(|dataset: &StorageBenchmarkDataset| {
                dataset.filtered_task_count as i64
            }),
        },
        StorageReadBenchmark {
            name: "exact document address among many",
            run: Box::new(|storage: &dyn Storage, dataset: &StorageBenchmarkDataset| {
                Ok(storage
                    .find_document(
                        &crate::durable::types::DocumentAddress {
                            kind: String::from("benchmark.family"),
                            scope: DocumentScope::Session,
                            key: Some(dataset.exact_document_key.clone()),
                        },
                        DocumentPoint::Current,
                        &background_context(),
                    )?
                    .map(|record| record.id)
                    .unwrap_or(-1))
            }),
            expected: Box::new(|dataset: &StorageBenchmarkDataset| dataset.exact_document_id),
        },
    ];
    for tail in REPLAY_TAILS {
        benchmarks.push(StorageReadBenchmark {
            name: match tail {
                0 => "document replay tail (0)",
                16 => "document replay tail (16)",
                128 => "document replay tail (128)",
                _ => "document replay tail (1024)",
            },
            run: Box::new(
                move |storage: &dyn Storage, dataset: &StorageBenchmarkDataset| {
                    let document = storage
                        .document(
                            dataset.replay_document_id(tail),
                            DocumentPoint::Current,
                            &background_context(),
                        )?
                        .expect("replay document exists");
                    Ok(document
                        .value
                        .get("count")
                        .and_then(|value| value.as_i64())
                        .unwrap_or_default())
                },
            ),
            expected: Box::new(move |_dataset| tail as i64),
        });
    }
    benchmarks.push(StorageReadBenchmark {
        name: "ancient historical read before newer base",
        run: Box::new(|storage: &dyn Storage, dataset: &StorageBenchmarkDataset| {
            let document = storage
                .document(
                    dataset.historical_document_id,
                    DocumentPoint::Seq(dataset.ancient_at),
                    &background_context(),
                )?
                .expect("historical document exists");
            Ok(document
                .value
                .get("count")
                .and_then(|value| value.as_i64())
                .unwrap_or_default())
        }),
        expected: Box::new(|_dataset: &StorageBenchmarkDataset| HISTORY_SEGMENT_LENGTH as i64),
    });
    benchmarks.push(StorageReadBenchmark {
        name: "recent historical read after newer base",
        run: Box::new(|storage: &dyn Storage, dataset: &StorageBenchmarkDataset| {
            let document = storage
                .document(
                    dataset.historical_document_id,
                    DocumentPoint::Seq(dataset.recent_at),
                    &background_context(),
                )?
                .expect("historical document exists");
            Ok(document
                .value
                .get("count")
                .and_then(|value| value.as_i64())
                .unwrap_or_default())
        }),
        expected: Box::new(|_dataset: &StorageBenchmarkDataset| {
            (HISTORY_SEGMENT_LENGTH * 2) as i64
        }),
    });
    benchmarks.push(StorageReadBenchmark {
        name: "fork-depth history scan (100)",
        run: Box::new(|storage: &dyn Storage, dataset: &StorageBenchmarkDataset| {
            Ok(storage
                .scan_entries(
                    crate::durable::types::EntryQuery {
                        conversation_id: dataset.deepest_conversation_id,
                        min_entry_id: None,
                        max_entry_id: None,
                    },
                    100,
                    None,
                    &background_context(),
                )?
                .items
                .len() as i64)
        }),
        expected: Box::new(|_dataset: &StorageBenchmarkDataset| 100),
    });
    benchmarks.push(StorageReadBenchmark {
        name: "fork-depth head lookup",
        run: Box::new(|storage: &dyn Storage, dataset: &StorageBenchmarkDataset| {
            Ok(storage
                .find_latest_head_marker(
                    dataset.deepest_conversation_id,
                    None,
                    &background_context(),
                )?
                .map(|marker| marker.id)
                .unwrap_or(-1))
        }),
        expected: Box::new(|dataset: &StorageBenchmarkDataset| dataset.ancestor_head_entry_id),
    });
    benchmarks
}

/// `StorageWriteBenchmark` (`testing/storage-benchmark.ts`).
pub struct StorageWriteBenchmark {
    pub name: &'static str,
    pub expected: i64,
    pub run: WriteBenchmarkRun,
}
/// `seedStorageWriteBenchmark(storage)` (`testing/storage-benchmark.ts`):
/// seed the common state expected by every write benchmark sample.
pub fn seed_storage_write_benchmark(
    storage: &dyn Storage,
) -> Result<(), crate::durable::storage::StorageError> {
    let context = background_context();
    storage.commit(
        &[StorageWrite::Conversation {
            value: ConversationRecord {
                id: ROOT_CONVERSATION_ID,
                parent: None,
                owner: None,
            },
        }],
        &context,
    )?;
    let mut writes = Vec::new();
    for index in 0..100 {
        writes.push(StorageWrite::Entry {
            value: EntryRecord {
                kind: String::from("benchmark.baseline"),
                model: None,
                data: Some(json!({ "index": index })),
                edits: None,
                id: storage.mint_id()?,
                conversation_id: ROOT_CONVERSATION_ID,
                head: None,
                by_task_id: None,
            },
        });
    }
    storage.commit(&writes, &context)?;
    Ok(())
}

/// One benchmark step's outcome.
pub type BenchmarkResult = Result<i64, crate::durable::storage::StorageError>;

/// One read-benchmark step over the seeded dataset.
pub type ReadBenchmarkRun = Box<dyn Fn(&dyn Storage, &StorageBenchmarkDataset) -> BenchmarkResult>;
/// One write-benchmark step over freshly seeded state.
pub type WriteBenchmarkRun = Box<dyn Fn(&dyn Storage) -> BenchmarkResult>;

/// `STORAGE_WRITE_BENCHMARKS` (`testing/storage-benchmark.ts`).
pub fn storage_write_benchmarks() -> Vec<StorageWriteBenchmark> {
    vec![
        StorageWriteBenchmark {
            name: "commit one entry",
            expected: 1,
            run: Box::new(|storage: &dyn Storage| -> BenchmarkResult {
                let id = storage.mint_id()?;
                storage.commit(
                    &[StorageWrite::Entry {
                        value: EntryRecord {
                            kind: String::from("benchmark.write"),
                            model: None,
                            data: Some(json!({ "text": "x".repeat(128) })),
                            edits: None,
                            id,
                            conversation_id: ROOT_CONVERSATION_ID,
                            head: None,
                            by_task_id: None,
                        },
                    }],
                    &background_context(),
                )?;
                Ok(1)
            }),
        },
        StorageWriteBenchmark {
            name: "commit 100 entries",
            expected: 100,
            run: Box::new(|storage: &dyn Storage| -> BenchmarkResult {
                let mut writes = Vec::new();
                for index in 0..100 {
                    writes.push(StorageWrite::Entry {
                        value: EntryRecord {
                            kind: String::from("benchmark.write"),
                            model: None,
                            data: Some(json!({ "index": index, "text": "x".repeat(128) })),
                            edits: None,
                            id: storage.mint_id()?,
                            conversation_id: ROOT_CONVERSATION_ID,
                            head: None,
                            by_task_id: None,
                        },
                    });
                }
                let length = writes.len() as i64;
                storage.commit(&writes, &background_context())?;
                Ok(length)
            }),
        },
        StorageWriteBenchmark {
            name: "commit mixed entry/task/submission/document",
            expected: 4,
            run: Box::new(|storage: &dyn Storage| -> BenchmarkResult {
                let entry_id = storage.mint_id()?;
                let task_id = storage.mint_id()?;
                let submission_id = storage.mint_id()?;
                let document_id = storage.mint_id()?;
                let writes = vec![
                    StorageWrite::Entry {
                        value: EntryRecord {
                            kind: String::from("benchmark.mixed"),
                            model: None,
                            data: None,
                            edits: None,
                            id: entry_id,
                            conversation_id: ROOT_CONVERSATION_ID,
                            head: None,
                            by_task_id: None,
                        },
                    },
                    StorageWrite::Task {
                        value: task(task_id, task_id as usize),
                    },
                    StorageWrite::Submission {
                        value: SubmissionRecord::settled_direct(
                            ROOT_CONVERSATION_ID,
                            Some(format!("benchmark-{submission_id}")),
                            SubmissionType::Write,
                            SubmissionStatus::Done,
                            entry_id,
                            submission_id,
                        ),
                    },
                    StorageWrite::DocumentCreate {
                        record: DocumentCreate {
                            id: document_id,
                            kind: String::from("benchmark.mixed"),
                            key: Some(document_id.to_string()),
                            scope: DocumentScope::Session,
                            history: None,
                            fork: None,
                        },
                        content: DocumentContent::Base {
                            version: 1,
                            value: json!({ "entryId": entry_id, "taskId": task_id })
                                .as_object()
                                .cloned()
                                .unwrap_or_default(),
                        },
                    },
                ];
                let length = writes.len() as i64;
                storage.commit(&writes, &background_context())?;
                Ok(length)
            }),
        },
    ]
}
