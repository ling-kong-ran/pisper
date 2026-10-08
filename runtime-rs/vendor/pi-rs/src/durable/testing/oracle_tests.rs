//! Byte-oracle tests against `tests/fixtures/durable_oracle/durable_oracle.json`
//! for the testing slice (`testing_conformance`, `testing_benchmark`),
//! captured by `capture_durable_oracle.mjs` from the read-only upstream
//! sources. Assertion traces are compared in the capture's canonical form
//! (object keys sorted, `undefined` → `null`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::storage_benchmark::{
    seed_storage_benchmark, seed_storage_write_benchmark, storage_benchmark_primary_record_count,
    storage_read_benchmarks, storage_write_benchmarks, StorageBenchmarkScale,
    STORAGE_MEMORY_SCALES, TIMING_SCALE,
};
use super::storage_conformance::create_storage_conformance;
use super::types::{
    ConformanceFailure, ConformanceResult, StorageConformanceAssertions, StorageConformanceOptions,
    StorageOperation,
};
use crate::durable::storage::{MemoryStorage, Storage};

fn oracle() -> Value {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = manifest_dir.join("tests/fixtures/durable_oracle/durable_oracle.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Canonical form shared with the capture: object keys sorted recursively.
fn canonical(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<(String, Value)> = map.into_iter().collect();
            keys.sort_by(|left, right| left.0.cmp(&right.0));
            let mut sorted = serde_json::Map::new();
            for (key, value) in keys {
                sorted.insert(key, canonical(value));
            }
            Value::Object(sorted)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canonical).collect()),
        other => other,
    }
}

fn wire(value: &Value) -> String {
    serde_json::to_string(value).unwrap()
}

const CONFORMANCE_CASES: [&str; 13] = [
    "reserves ID 1 for the immutable root conversation",
    "commits mixed table writes atomically and rolls all of them back on failure",
    "detaches retained writes and every returned record",
    "detaches prototype-like JSON keys without changing object prototypes",
    "indexes entries committed out of ID order",
    "continues an entry cursor below its last item after a newer commit",
    "paginates conversations by opaque cursor in ascending ID order",
    "filters and pages conversations by durable owner edges",
    "replaces complete task records and pages filtered task scans",
    "stores owners and scans waiting and completing tasks by status",
    "indexes logical addresses and exact-scope scans independently",
    "keeps one global record ID namespace and rejects exhausted ID minting",
    "rejects every operation after close",
];

/// The capture's recording assertions: every call appends a canonical step and
/// a failed assertion resolves to the fixed failure text.
struct RecordingAssertions {
    steps: Mutex<Vec<Value>>,
}

impl RecordingAssertions {
    fn new() -> Arc<Self> {
        Arc::new(RecordingAssertions {
            steps: Mutex::new(Vec::new()),
        })
    }

    fn push(&self, step: Value) {
        self.steps
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(step);
    }

    fn steps(&self) -> Vec<Value> {
        self.steps
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn fail() -> ConformanceFailure {
        ConformanceFailure::new("conformance assertion failed")
    }
}

impl StorageConformanceAssertions for RecordingAssertions {
    fn ok(&self, value: bool, message: &str) -> ConformanceResult {
        self.push(json!({ "method": "ok", "value": value, "message": message }));
        if value {
            Ok(())
        } else {
            Err(Self::fail())
        }
    }

    fn strict_equal(&self, actual: &Value, expected: &Value) -> ConformanceResult {
        self.push(json!({ "method": "strictEqual", "actual": actual, "expected": expected }));
        if actual == expected {
            Ok(())
        } else {
            Err(Self::fail())
        }
    }

    fn deep_equal(&self, actual: &Value, expected: &Value) -> ConformanceResult {
        self.push(json!({ "method": "deepEqual", "actual": actual, "expected": expected }));
        if actual == expected {
            Ok(())
        } else {
            Err(Self::fail())
        }
    }

    fn partial_deep_equal(&self, actual: &Value, expected: &Value) -> ConformanceResult {
        self.push(json!({ "method": "partialDeepEqual", "actual": actual, "expected": expected }));
        if partial_equal(actual, expected) {
            Ok(())
        } else {
            Err(Self::fail())
        }
    }

    fn greater_than(&self, actual: i64, expected: i64) -> ConformanceResult {
        self.push(json!({ "method": "greaterThan", "actual": actual, "expected": expected }));
        if actual > expected {
            Ok(())
        } else {
            Err(Self::fail())
        }
    }

    fn rejects(&self, operation: StorageOperation, message_includes: &str) -> ConformanceResult {
        let matched = match operation() {
            Ok(()) => false,
            Err(error) => error.to_string().contains(message_includes),
        };
        self.push(json!({
            "method": "rejects",
            "messageIncludes": message_includes,
            "matched": matched,
        }));
        if matched {
            Ok(())
        } else {
            Err(Self::fail())
        }
    }
}

/// The capture's `partialEqual`: subset match for object expectations.
fn partial_equal(actual: &Value, expected: &Value) -> bool {
    match expected {
        Value::Object(expected_fields) => match actual {
            Value::Object(actual_fields) => expected_fields.iter().all(|(key, expected_value)| {
                actual_fields
                    .get(key)
                    .is_some_and(|actual_value| partial_equal(actual_value, expected_value))
            }),
            _ => false,
        },
        _ => actual == expected,
    }
}

#[test]
fn oracle_testing_conformance() {
    let expected = oracle()["testing_conformance"].clone();
    let mut records = Vec::new();
    for name in CONFORMANCE_CASES {
        let recorder = RecordingAssertions::new();
        let with_storage: super::types::StorageConformanceProvider = Arc::new(|test| {
            let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
            test(storage)
        });
        let options = StorageConformanceOptions {
            assertions: recorder.clone(),
            with_storage,
        };
        let cases = create_storage_conformance(&options);
        let case = cases
            .iter()
            .find(|case| case.name == name)
            .unwrap_or_else(|| panic!("missing conformance case: {name}"));
        let error = match case.run(&options) {
            Ok(()) => Value::Null,
            Err(failure) => Value::String(failure.message),
        };
        records.push(json!({
            "name": name,
            "steps": recorder
                .steps()
                .into_iter()
                .map(canonical)
                .collect::<Vec<_>>(),
            "error": error,
        }));
    }
    assert_eq!(
        wire(&Value::Array(records)),
        wire(&expected),
        "conformance traces"
    );
}

#[test]
fn oracle_testing_benchmark() {
    let expected = oracle()["testing_benchmark"].clone();
    let tiny = StorageBenchmarkScale {
        name: "tiny",
        entry_count: 8,
        task_count: 6,
        document_count: 4,
    };

    let scales = json!([
        {
            "name": STORAGE_MEMORY_SCALES[0].name,
            "entryCount": STORAGE_MEMORY_SCALES[0].entry_count,
            "taskCount": STORAGE_MEMORY_SCALES[0].task_count,
            "documentCount": STORAGE_MEMORY_SCALES[0].document_count,
        },
        {
            "name": STORAGE_MEMORY_SCALES[1].name,
            "entryCount": STORAGE_MEMORY_SCALES[1].entry_count,
            "taskCount": STORAGE_MEMORY_SCALES[1].task_count,
            "documentCount": STORAGE_MEMORY_SCALES[1].document_count,
        },
    ]);
    let timing = json!({
        "name": TIMING_SCALE.name,
        "entryCount": TIMING_SCALE.entry_count,
        "taskCount": TIMING_SCALE.task_count,
        "documentCount": TIMING_SCALE.document_count,
    });
    let primary = json!({
        "scale1k": storage_benchmark_primary_record_count(&STORAGE_MEMORY_SCALES[0]),
        "timing": storage_benchmark_primary_record_count(&TIMING_SCALE),
        "tiny": storage_benchmark_primary_record_count(&tiny),
    });

    let bench_storage = MemoryStorage::new();
    let dataset = seed_storage_benchmark(&bench_storage, &tiny).unwrap();
    // The capture canonicalizes the dataset, but JS serializes integer-like
    // object keys in ascending numeric order regardless of insertion, so
    // `replayDocumentIds` keeps its numeric key order.
    let mut dataset_fields = dataset.to_json();
    dataset_fields.remove("replayDocumentIds");
    let mut dataset_map = canonical(Value::Object(dataset_fields));
    let mut replay_map = serde_json::Map::new();
    for (tail, id) in dataset.replay_document_ids {
        replay_map.insert(json!(tail).to_string(), json!(id));
    }
    dataset_map
        .as_object_mut()
        .unwrap()
        .insert(String::from("replayDocumentIds"), Value::Object(replay_map));
    let dataset_json = dataset_map;

    let mut read_rows = Vec::new();
    for benchmark in storage_read_benchmarks() {
        let run = (benchmark.run)(&bench_storage, &dataset).unwrap();
        read_rows.push(json!({
            "name": benchmark.name,
            "run": run,
            "expected": (benchmark.expected)(&dataset),
        }));
    }

    let write_storage = MemoryStorage::new();
    seed_storage_write_benchmark(&write_storage).unwrap();
    let mut write_rows = Vec::new();
    for benchmark in storage_write_benchmarks() {
        let run = (benchmark.run)(&write_storage).unwrap();
        write_rows.push(json!({
            "name": benchmark.name,
            "expected": benchmark.expected,
            "run": run,
        }));
    }

    let computed = json!({
        "memoryScales": canonical(scales),
        "timingScale": canonical(timing),
        "primaryRecordCounts": primary,
        "dataset": dataset_json,
        "readBenchmarks": read_rows,
        "writeBenchmarks": write_rows,
    });
    assert_eq!(
        wire(&canonical(computed.clone())),
        wire(&canonical(expected.clone())),
        "benchmark values"
    );
    assert_eq!(
        wire(&computed),
        wire(&expected),
        "benchmark construction order"
    );
}
