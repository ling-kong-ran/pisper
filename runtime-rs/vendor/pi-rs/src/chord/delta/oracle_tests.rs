//! Oracle-driven tests for the chord delta port. Expected values were
//! captured from the read-only upstream TypeScript sources
//! (`pi/packages/chord/src/delta` + `json.ts` + `api.ts` + `services`) run
//! under `node --experimental-strip-types` by
//! `tests/fixtures/chord_delta_oracle/capture_chord_delta.mjs` and stored in
//! `tests/fixtures/chord_delta_oracle/chord_delta_oracle.json`. Comparison is
//! byte-identical over the canonical serialization (compact JSON with
//! recursively sorted object keys), matching the capture script's `canon`.

use serde_json::{json, Value};

use super::tracker::{TrackerError, TrackerErrorKind};
use super::{
    apply, apply_immutable, apply_immutable_batches, decoder, diff_revisions, encoder, is_base,
    op_from_json, op_to_json, overlap, track, wire_op_from_json, wire_op_to_json, Change, Op,
    Prepared, Seg, Tracker, WireOp,
};
use crate::chord::types::JsonValue;

const ORACLE: &str =
    include_str!("../../../tests/fixtures/chord_delta_oracle/chord_delta_oracle.json");

fn canon(value: &JsonValue) -> String {
    // Both Node capture scripts explicitly canonicalize comparison values.
    // Sort a clone only; production state and wire retain insertion order.
    let mut sorted = value.clone();
    sorted.sort_all_objects();
    serde_json::to_string(&sorted).expect("canonical serialization cannot fail")
}

fn seg_from_json(value: &Value) -> Seg {
    if let Some(key) = value.as_str() {
        Seg::Key(key.to_owned())
    } else {
        Seg::Index(value.as_u64().expect("path segment") as usize)
    }
}

fn path_from_json(value: &Value) -> super::Path {
    value
        .as_array()
        .expect("path")
        .iter()
        .map(seg_from_json)
        .collect()
}

fn ops_value(ops: &[Op]) -> JsonValue {
    JsonValue::Array(ops.iter().map(op_to_json).collect())
}

/// Resolve a path against a plain value; a miss is upstream `undefined`,
/// serialized as `null` by the capture.
fn resolve_value(value: &JsonValue, path: &[Seg]) -> JsonValue {
    let mut at = value;
    for segment in path {
        match (at, segment) {
            (JsonValue::Object(object), Seg::Key(key)) => match object.get(key) {
                Some(next) => at = next,
                None => return JsonValue::Null,
            },
            (JsonValue::Array(array), Seg::Index(index)) => match array.get(*index) {
                Some(next) => at = next,
                None => return JsonValue::Null,
            },
            _ => return JsonValue::Null,
        }
    }
    at.clone()
}

/// The scenario interpreter: mirrors
/// `capture_chord_delta.mjs` `runTrackerScenario` step-for-step over the
/// port's path-addressed draft API.
struct Interpreter {
    tracker: Tracker,
    change: Option<Change>,
    change_settled: bool,
    prepared: [Option<Prepared>; 2],
    steps: Vec<Value>,
}

impl Interpreter {
    fn new(init: &JsonValue) -> Interpreter {
        Interpreter {
            tracker: track(init.clone()),
            change: None,
            change_settled: false,
            prepared: [None, None],
            steps: Vec::new(),
        }
    }

    /// Upstream retains the draft proxies after prepare; they throw
    /// "Cannot use a settled overlay" once the overlay is cleared. The
    /// port's handle is consumed by `prepare`, so the same steps reproduce
    /// the same error text.
    fn draft(&mut self) -> Result<&mut Change, TrackerError> {
        if self.change.is_none() {
            return Err(TrackerError(TrackerErrorKind::Type(
                "Cannot use a settled overlay".to_owned(),
            )));
        }
        Ok(self.change.as_mut().expect("checked above"))
    }

    fn record(&mut self, op: &str, result: Result<Option<JsonValue>, TrackerError>) {
        let mut outcome = json!({ "step": self.steps.len(), "op": op });
        match result {
            Ok(Some(payload)) => {
                if let Some(fields) = payload.as_object() {
                    for key in ["ops", "value", "base_revision"] {
                        if let Some(field) = fields.get(key) {
                            outcome[key] = field.clone();
                        }
                    }
                }
            }
            Ok(None) => {}
            Err(error) => {
                outcome["error"] = json!(format!("{}: {}", error.kind(), error.message()));
            }
        }
        self.steps.push(outcome);
    }

    fn run_step(&mut self, step: &Value) {
        let op = step["op"].as_str().expect("step op").to_owned();
        let slot = step.get("slot").and_then(Value::as_u64).unwrap_or(0) as usize;
        let outcome = self.dispatch(&op, slot, step);
        self.record(&op, outcome);
    }

    #[allow(clippy::too_many_lines)]
    fn dispatch(
        &mut self,
        op: &str,
        slot: usize,
        step: &Value,
    ) -> Result<Option<JsonValue>, TrackerError> {
        match op {
            "begin_change" | "begin_change_again" => {
                self.change = Some(self.tracker.begin_change());
                self.change_settled = false;
                Ok(None)
            }
            "set" => {
                let value = step["value"].clone();
                self.draft()?.set(&path_from_json(&step["path"]), value)?;
                Ok(None)
            }
            "delete" => {
                self.draft()?.delete(&path_from_json(&step["path"]))?;
                Ok(None)
            }
            "push" => {
                let items = step["items"].as_array().expect("items").clone();
                self.draft()?.push(&path_from_json(&step["path"]), items)?;
                Ok(None)
            }
            "pop" => {
                self.draft()?.pop(&path_from_json(&step["path"]))?;
                Ok(None)
            }
            "shift" => {
                self.draft()?.shift(&path_from_json(&step["path"]))?;
                Ok(None)
            }
            "unshift" => {
                let items = step["items"].as_array().expect("items").clone();
                self.draft()?
                    .unshift(&path_from_json(&step["path"]), items)?;
                Ok(None)
            }
            "splice" => {
                let items = step
                    .get("items")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                self.draft()?.splice(
                    &path_from_json(&step["path"]),
                    step["start"].as_i64().expect("start") as isize,
                    step["remove"].as_i64().expect("remove") as isize,
                    items,
                )?;
                Ok(None)
            }
            "set_length" => {
                self.draft()?.set_length(
                    &path_from_json(&step["path"]),
                    step["n"].as_u64().expect("n") as usize,
                )?;
                Ok(None)
            }
            "reverse" => {
                self.draft()?.reverse(&path_from_json(&step["path"]))?;
                Ok(None)
            }
            "sort_default" => {
                self.draft()?.sort_default(&path_from_json(&step["path"]))?;
                Ok(None)
            }
            "fill" => {
                self.draft()?.fill(
                    &path_from_json(&step["path"]),
                    step["value"].clone(),
                    Some(step["start"].as_i64().expect("start") as isize),
                    Some(step["end"].as_i64().expect("end") as isize),
                )?;
                Ok(None)
            }
            "copy_within" => {
                self.draft()?.copy_within(
                    &path_from_json(&step["path"]),
                    step["target"].as_i64().expect("target") as isize,
                    step["start"].as_i64().expect("start") as isize,
                    Some(step["end"].as_i64().expect("end") as isize),
                )?;
                Ok(None)
            }
            "read_draft" => {
                let value = self
                    .draft()?
                    .read(&step.get("path").map(path_from_json).unwrap_or_default())?;
                Ok(Some(json!({ "value": value.unwrap_or(JsonValue::Null) })))
            }
            "read" => {
                let value = resolve_value(
                    &self.tracker.value(),
                    &step.get("path").map(path_from_json).unwrap_or_default(),
                );
                Ok(Some(json!({ "value": value })))
            }
            "prepare" => {
                let change = self.change.take().ok_or_else(|| {
                    TrackerError(TrackerErrorKind::Type(
                        "Cannot use a settled overlay".to_owned(),
                    ))
                })?;
                self.change_settled = true;
                let prepared = change.prepare()?;
                let record = json!({
                    "ops": ops_value(&prepared.ops),
                    "value": prepared.value,
                    "base_revision": prepared.base_revision,
                });
                self.prepared[slot] = Some(prepared);
                Ok(Some(record))
            }
            "adopt" => {
                // The upstream prepared object stays usable after a failed
                // adoption; a clone models that here.
                let prepared = self
                    .prepared
                    .get(slot)
                    .expect("slot")
                    .clone()
                    .ok_or_else(|| {
                        TrackerError(TrackerErrorKind::Plain(
                            "Prepared change is not ready".to_owned(),
                        ))
                    })?;
                self.tracker.adopt(prepared)?;
                Ok(None)
            }
            "abort" => {
                if let Some(prepared) = self.prepared.get_mut(slot).expect("slot").take() {
                    prepared.abort();
                } else if let Some(change) = self.change.take() {
                    change.abort();
                    self.change_settled = false;
                }
                Ok(None)
            }
            "prepare_replace" => {
                let prepared = self.tracker.prepare_replace(step["value"].clone())?;
                let record = json!({
                    "ops": ops_value(&prepared.ops),
                    "value": prepared.value,
                    "base_revision": prepared.base_revision,
                });
                self.prepared[slot] = Some(prepared);
                Ok(Some(record))
            }
            "flag_revision" => Ok(Some(json!({ "value": self.tracker.revision() }))),
            other => panic!("unknown step {other}"),
        }
    }
}

#[test]
fn canonical_comparison_preserves_original_value_order() {
    let raw = r#"{"z":[{"y":1,"a":2}],"a":3}"#;
    let value: JsonValue = serde_json::from_str(raw).unwrap();
    assert_eq!(canon(&value), r#"{"a":3,"z":[{"a":2,"y":1}]}"#);
    assert_eq!(serde_json::to_string(&value).unwrap(), raw);
}

#[test]
fn overlap_matches_upstream_probing() {
    // "bc" is the longest suffix of the left that prefixes the right.
    assert_eq!(overlap("abc", "bcd", 100), 2);
    assert_eq!(overlap("xxab", "abyy", 100), 2);
    assert_eq!(overlap("", "a", 100), 0);
    assert_eq!(overlap("abc", "abc", 0), 0);
}

#[test]
fn op_validation_rejects_wire_forms() {
    // Wire-only shapes fail `assertValidOp`, exactly as upstream.
    for raw in [
        json!(["s", "value"]),
        json!(["d"]),
        json!(["a", "text"]),
        json!(["t", 2]),
        json!(["p", 0, 0, []]),
        json!(["#", 0, ["a"]]),
        json!(["s", ["__proto__", "isAdmin"], true]),
        json!(["s", [], 1]),
        json!(["m", ["xs"], [0, 0]]),
        json!(["unknown"]),
    ] {
        assert!(op_from_json(&raw).is_err(), "{raw} must fail");
    }
    for raw in [
        json!(["r", null]),
        json!(["s", ["a", 0], 1]),
        json!(["d", ["a"]]),
        json!(["a", ["s"], "x"]),
        json!(["t", ["s"], 1]),
        json!(["p", [], 0, 0, []]),
        json!(["m", [], [1, 0]]),
    ] {
        assert!(op_from_json(&raw).is_ok(), "{raw} must parse");
    }
}

#[test]
fn codec_roundtrips_ops() {
    let ops = vec![
        Op::Replace(json!({ "b": 1, "a": [true, null] })),
        Op::Set {
            path: vec![Seg::Key("a".into()), Seg::Index(0)],
            value: json!(1),
        },
        Op::Delete {
            path: vec![Seg::Key("a".into())],
        },
        Op::Append {
            path: vec![Seg::Key("s".into())],
            text: "text".into(),
        },
        Op::Truncate {
            path: vec![Seg::Key("s".into())],
            count: 2,
        },
        Op::Splice {
            path: super::Path::new(),
            index: 1,
            remove: 1,
            items: vec![json!("x")],
        },
        Op::Reorder {
            path: super::Path::new(),
            permutation: vec![2, 0, 1],
        },
    ];
    let mut enc = encoder();
    let wire = enc.encode(&ops);
    let mut dec = decoder();
    let decoded = dec.decode(&wire).expect("tracker ops always decode");
    assert_eq!(decoded, ops);
    // Wire tuples serialize exactly.
    for wire_op in &wire {
        let json = wire_op_to_json(wire_op);
        let parsed: WireOp = wire_op_from_json(&json).expect("wire tuple revalidates");
        assert_eq!(&parsed, wire_op);
    }
}

#[test]
fn apply_matches_upstream_semantics() {
    let target = json!({ "a": [1, 2, 3], "s": "ab" });
    let ops = vec![
        Op::Splice {
            path: vec![Seg::Key("a".into())],
            index: 1,
            remove: 1,
            items: vec![json!("x")],
        },
        Op::Append {
            path: vec![Seg::Key("s".into())],
            text: "cd".into(),
        },
    ];
    let applied = apply(Some(&target), &ops).expect("applies");
    assert_eq!(applied["a"], json!([1, "x", 3]));
    assert_eq!(applied["s"], json!("abcd"));
    // A splice past the end appends; a removal clamps to the tail.
    let clamped = apply(
        Some(&json!([1, 2])),
        &[Op::Splice {
            path: super::Path::new(),
            index: 9,
            remove: 5,
            items: vec![json!(3)],
        }],
    )
    .expect("applies");
    assert_eq!(clamped, json!([1, 2, 3]));
}

#[test]
fn apply_immutable_batches_replays() {
    let ops: Vec<Op> = vec![Op::Set {
        path: vec![Seg::Key("n".into())],
        value: json!(1),
    }];
    let target = json!({ "n": 0 });
    let result =
        apply_immutable_batches(Some(&target), vec![ops.clone(), ops]).expect("batches apply");
    assert_eq!(result, json!({ "n": 1 }));
    assert_eq!(target, json!({ "n": 0 }), "input is untouched");
}

#[test]
fn is_base_and_is_replace() {
    assert!(is_base(&[Op::Replace(json!(null))]));
    assert!(!is_base(&[]));
    assert!(super::is_replace(&Op::Replace(json!(1))));
}

#[test]
fn diff_revisions_oracle_pairs() {
    let oracle: Value = serde_json::from_str(ORACLE).expect("oracle fixture parses");
    for pair in oracle["diffs"].as_array().expect("diff rows") {
        let before = pair["before"].clone();
        let after = pair["after"].clone();
        let ops = diff_revisions(&before, &after);
        assert_eq!(
            canon(&ops_value(&ops)),
            pair["ops"].as_str().expect("canonical ops"),
            "diff pair {}",
            pair["name"].as_str().expect("name")
        );
    }
}

#[test]
fn apply_oracle_cases() {
    let oracle: Value = serde_json::from_str(ORACLE).expect("oracle fixture parses");
    for item in oracle["applies"].as_array().expect("apply rows") {
        let input: Value = serde_json::from_str(item["input"].as_str().unwrap()).unwrap();
        let ops: Vec<Op> = input["ops"]
            .as_array()
            .expect("ops")
            .iter()
            .map(|op| op_from_json(op).expect("valid op"))
            .collect();
        let target: Option<JsonValue> = if input["target"].is_null() {
            None
        } else {
            Some(input["target"].clone())
        };
        let applied = apply(target.as_ref(), &ops).expect("applies");
        assert_eq!(canon(&applied), item["applied"], "apply case {item}");
        let immutable = apply_immutable(target.as_ref(), &ops).expect("applies immutably");
        assert_eq!(
            canon(&immutable),
            item["immutable"],
            "immutable case {item}"
        );
    }
}

#[test]
fn tracker_oracle_scenarios() {
    let oracle: Value = serde_json::from_str(ORACLE).expect("oracle fixture parses");
    for scenario in oracle["tracker"].as_array().expect("tracker rows") {
        let mut interpreter = Interpreter::new(&scenario["init"]);
        let steps = scenario["steps"].as_array().expect("steps").clone();
        for step in &steps {
            interpreter.run_step(step);
        }
        for (at, expected) in scenario["outcomes"]
            .as_array()
            .expect("outcomes")
            .iter()
            .enumerate()
        {
            let actual = &interpreter.steps[at];
            assert_eq!(
                actual["op"], expected["op"],
                "{}: step {at} op",
                scenario["name"]
            );
            match expected.get("error") {
                Some(error) => assert_eq!(
                    actual["error"].as_str(),
                    Some(error.as_str().expect("error text")),
                    "{}: step {at} error",
                    scenario["name"]
                ),
                None => assert!(
                    actual.get("error").is_none(),
                    "{}: step {at} unexpected error {:?}",
                    scenario["name"],
                    actual["error"]
                ),
            }
            if let Some(ops) = expected.get("ops") {
                assert_eq!(
                    canon(&actual["ops"]),
                    ops.as_str().expect("canonical ops"),
                    "{}: step {at} ops",
                    scenario["name"]
                );
            }
            if let Some(value) = expected.get("value") {
                let actual_value = actual.get("value").map(canon).unwrap_or_default();
                let expected_value = if value.is_string() {
                    value.as_str().expect("string").to_owned()
                } else {
                    canon(value)
                };
                assert_eq!(
                    actual_value, expected_value,
                    "{}: step {at} value",
                    scenario["name"]
                );
            }
            if let Some(base_revision) = expected.get("base_revision") {
                assert_eq!(
                    actual["base_revision"],
                    json!(base_revision.as_u64().expect("revision")),
                    "{}: step {at} base_revision",
                    scenario["name"]
                );
            }
        }
        assert_eq!(
            canon(&interpreter.tracker.value()),
            scenario["final"].as_str().expect("final"),
            "{}: final value",
            scenario["name"]
        );
        assert_eq!(
            interpreter.tracker.revision(),
            scenario["revision"].as_u64().expect("revision"),
            "{}: revision",
            scenario["name"]
        );
    }
}
