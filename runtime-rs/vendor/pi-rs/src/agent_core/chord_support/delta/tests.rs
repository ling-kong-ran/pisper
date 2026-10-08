//! Semantics tests for the chord delta port, derived from
//! `packages/chord/test/delta.test.ts` and the behavior of
//! `packages/chord/src/delta/index.ts`. Cases whose pinned op *shapes* depend
//! on upstream's write-time recording (rather than on its flush-side diff
//! functions, which this port reuses) assert convergence —
//! `apply(prev, flush()) == state` — instead of exact tuples, per the
//! contract in `packages/chord/src/delta/README.md` ("The operation sequence
//! is not canonical ... consumers must depend on the resulting value").

use super::*;
use serde_json::json;

// ─── helpers ────────────────────────────────────────────────────────────────

fn k(key: &str) -> Seg {
    Seg::Key(key.to_owned())
}

fn i(index: usize) -> Seg {
    Seg::Index(index)
}

fn replace(value: JsonValue) -> Op {
    Op::Replace(value)
}

fn set(path: &[Seg], value: JsonValue) -> Op {
    Op::Set {
        path: path.to_vec(),
        value,
    }
}

fn delete(path: &[Seg]) -> Op {
    Op::Delete {
        path: path.to_vec(),
    }
}

fn append(path: &[Seg], text: &str) -> Op {
    Op::Append {
        path: path.to_vec(),
        text: text.to_owned(),
    }
}

fn truncate(path: &[Seg], count: usize) -> Op {
    Op::Truncate {
        path: path.to_vec(),
        count,
    }
}

fn splice(path: &[Seg], index: usize, remove: usize, items: Vec<JsonValue>) -> Op {
    Op::Splice {
        path: path.to_vec(),
        index,
        remove,
        items,
    }
}

fn apply_ok(target: Option<&JsonValue>, ops: &[Op]) -> JsonValue {
    apply_immutable(target, ops).expect("ops apply")
}

fn apply_err(target: Option<&JsonValue>, ops: &[Op]) -> DeltaError {
    apply_immutable(target, ops).expect_err("ops must fail")
}

/// Publish-and-fold helper mirroring a replica: `None` until the first batch.
fn fold(replica: &mut Option<JsonValue>, ops: &[Op]) {
    *replica = Some(apply_ok(replica.as_ref(), ops));
}

fn sorted(mut ops: Vec<Op>) -> Vec<Op> {
    ops.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    ops
}

// ─── applyImmutable: replacement, paths, base ───────────────────────────────

#[test]
fn replace_seeds_an_undefined_replica() {
    // `r` replaces outright and cannot be done in place on undefined
    // (delta/index.ts:1361-1370).
    let out = apply_ok(None, &[replace(json!({ "x": 0 }))]);
    assert_eq!(out, json!({ "x": 0 }));
}

#[test]
fn replace_adopts_among_later_ops_without_mutating_the_payload() {
    // "does not mutate a replacement payload targeted by a later operation"
    // (delta.test.ts:575-584).
    let replacement = json!({ "nested": { "value": 1 } });
    let next = apply_ok(
        None,
        &[
            replace(replacement.clone()),
            set(&[k("nested"), k("value")], json!(2)),
        ],
    );
    assert_eq!(replacement, json!({ "nested": { "value": 1 } }));
    assert_eq!(next, json!({ "nested": { "value": 2 } }));
}

#[test]
fn apply_immutable_never_mutates_its_target() {
    let target = json!({ "a": 1 });
    let out = apply_ok(Some(&target), &[set(&[k("a")], json!(2))]);
    assert_eq!(target, json!({ "a": 1 }));
    assert_eq!(out, json!({ "a": 2 }));
}

#[test]
fn set_delete_append_truncate_round_trip() {
    let initial = json!({ "s": "abc", "o": { "x": 1 }, "xs": [1, 2, 3] });
    let ops = vec![
        set(&[k("o"), k("x")], json!(9)),
        append(&[k("s")], "def"),
        truncate(&[k("s")], 2),
        delete(&[k("o"), k("x")]),
        set(&[k("xs"), i(1)], json!(20)),
        splice(&[k("xs")], 0, 1, vec![json!(0)]),
    ];
    let out = apply_ok(Some(&initial), &ops);
    assert_eq!(out, json!({ "s": "cdef", "o": {}, "xs": [0, 20, 3] }));
}

#[test]
fn truncate_slices_off_the_string_front() {
    // `current.slice(op[2])` (delta/index.ts:1432-1436).
    let out = apply_ok(Some(&json!({ "s": "abcdef" })), &[truncate(&[k("s")], 4)]);
    assert_eq!(out, json!({ "s": "ef" }));
}

#[test]
fn truncate_count_past_the_length_yields_the_empty_string() {
    // JS slice(start >= length) is "".
    let out = apply_ok(Some(&json!({ "s": "ab" })), &[truncate(&[k("s")], 99)]);
    assert_eq!(out, json!({ "s": "" }));
}

#[test]
fn truncate_counts_utf16_code_units() {
    // README "Strings": `t` removes UTF-16 code units. A surrogate pair is two
    // units, so truncating 2 units of "ab😀cd" leaves "cd" even though it is
    // one char.
    let out = apply_ok(Some(&json!({ "s": "ab😀cd" })), &[truncate(&[k("s")], 4)]);
    assert_eq!(out, json!({ "s": "cd" }));
}

#[test]
fn sdat_cannot_target_the_root() {
    // "s/d/a/t can never target the root — the type forbids it"
    // (delta/index.ts:1403); assertValidOp requires non-empty paths.
    let err = apply_err(None, &[set(&[], json!(1))]);
    assert_eq!(err, DeltaError::InvalidOp("path is empty".to_owned()));
    assert!(matches!(
        apply_err(Some(&json!({})), &[delete(&[])]),
        DeltaError::InvalidOp(_)
    ));
    assert!(matches!(
        apply_err(Some(&json!({})), &[append(&[], "x")]),
        DeltaError::InvalidOp(_)
    ));
    assert!(matches!(
        apply_err(Some(&json!({})), &[truncate(&[], 1)]),
        DeltaError::InvalidOp(_)
    ));
}

#[test]
fn splice_may_target_a_root_array() {
    // "splices a value that is itself an array" (delta.test.ts:178-185).
    let out = apply_ok(
        Some(&json!([1, 2, 3])),
        &[splice(&[], 3, 0, vec![json!(4)])],
    );
    assert_eq!(out, json!([1, 2, 3, 4]));
}

#[test]
fn array_set_appends_exactly_one_past_the_end() {
    // delta.test.ts:908-913.
    let base = json!({ "xs": [1, 2, 3] });
    assert_eq!(
        apply_ok(Some(&base), &[set(&[k("xs"), i(3)], json!(9))]),
        json!({ "xs": [1, 2, 3, 9] })
    );
    assert_eq!(
        apply_ok(Some(&base), &[set(&[k("xs"), i(1)], json!(9))]),
        json!({ "xs": [1, 9, 3] })
    );
}

#[test]
fn array_set_rejects_a_gap() {
    // delta.test.ts:914-919: a sparse array does not survive a JSON round
    // trip, so the write is rejected rather than silently diverging.
    let err = apply_err(
        Some(&json!({ "xs": [1, 2, 3] })),
        &[set(&[k("xs"), i(5)], json!(9))],
    );
    assert!(matches!(err, DeltaError::UnsafePath { .. }));
    // "rejects a huge index": would otherwise allocate 4.29 billion entries.
    let err = apply_err(
        Some(&json!({ "xs": [] })),
        &[set(&[k("xs"), i(4_294_967_290)], json!(1))],
    );
    assert!(matches!(err, DeltaError::UnsafePath { .. }));
}

#[test]
fn rejects_string_spelled_array_indices() {
    // delta.test.ts:921-924.
    let err = apply_err(
        Some(&json!({ "xs": [1, 2, 3] })),
        &[set(&[k("xs"), k("7")], json!(9))],
    );
    assert!(matches!(err, DeltaError::UnsafePath { .. }));
    let err = apply_err(
        Some(&json!({ "xs": ["a"] })),
        &[append(&[k("xs"), k("0")], "b")],
    );
    assert!(matches!(err, DeltaError::UnsafePath { .. }));
}

#[test]
fn rejects_a_constructor_walk() {
    // delta.test.ts:817-820: ({}).constructor.constructor is Function — the
    // classic escape ladder. Reserved segments never resolve.
    let err = apply_err(
        Some(&json!({})),
        &[set(
            &[k("constructor"), k("prototype"), k("gadget")],
            json!(true),
        )],
    );
    assert!(matches!(err, DeltaError::UnsafePath { .. }));
}

#[test]
fn allows_a_reserved_name_as_a_value_key() {
    // delta.test.ts:844-849: reserved as segments, not as values.
    let out = apply_ok(
        Some(&json!({})),
        &[set(&[k("a")], json!({ "__proto__": { "z": 1 } }))],
    );
    assert_eq!(out["a"]["__proto__"], json!({ "z": 1 }));
}

#[test]
fn rejects_append_to_a_missing_or_non_string_value() {
    // delta.test.ts:980-983.
    assert!(matches!(
        apply_err(Some(&json!({ "a": 1 })), &[append(&[k("missing")], "x")]),
        DeltaError::UnresolvablePath { .. }
    ));
    assert!(matches!(
        apply_err(Some(&json!({ "a": 1 })), &[append(&[k("a")], "x")]),
        DeltaError::UnresolvablePath { .. }
    ));
}

#[test]
fn rejects_deleting_one_past_an_arrays_end() {
    // delta.test.ts:952-954.
    assert!(matches!(
        apply_err(Some(&json!({ "xs": [1] })), &[delete(&[k("xs"), i(1)])]),
        DeltaError::UnresolvablePath { .. }
    ));
}

#[test]
fn unresolvable_intermediate_paths_error() {
    // resolveValue requires an own property at every step
    // (delta/index.ts:1496-1507).
    assert!(matches!(
        apply_err(
            Some(&json!({ "a": {} })),
            &[set(&[k("a"), k("b"), k("c")], json!(1))]
        ),
        DeltaError::UnresolvablePath { .. }
    ));
    // A scalar cannot be walked through.
    assert!(matches!(
        apply_err(
            Some(&json!({ "a": 1 })),
            &[set(&[k("a"), k("b")], json!(1))]
        ),
        DeltaError::UnresolvablePath { .. }
    ));
}

#[test]
fn splice_clamps_remove_past_the_end() {
    // delta.test.ts:988-991: deterministic and identical on both sides.
    let out = apply_ok(
        Some(&json!({ "xs": [1, 2] })),
        &[splice(&[k("xs")], 0, 1_000_000_000, vec![])],
    );
    assert_eq!(out, json!({ "xs": [] }));
}

#[test]
fn splice_index_past_the_end_appends() {
    // JS splice(start > len) appends.
    let out = apply_ok(
        Some(&json!({ "xs": [1, 2, 3] })),
        &[splice(&[k("xs")], 10, 0, vec![json!(9)])],
    );
    assert_eq!(out, json!({ "xs": [1, 2, 3, 9] }));
}

#[test]
fn splice_replaces_and_shifts() {
    let out = apply_ok(
        Some(&json!({ "xs": [1, 2, 3, 4] })),
        &[splice(
            &[k("xs")],
            1,
            2,
            vec![json!("a"), json!("b"), json!("c")],
        )],
    );
    assert_eq!(out, json!({ "xs": [1, "a", "b", "c", 4] }));
}

#[test]
fn allows_explicit_growth_with_nulls() {
    // delta.test.ts:949-951.
    let out = apply_ok(
        Some(&json!({ "xs": [1] })),
        &[splice(
            &[k("xs")],
            1,
            0,
            vec![json!(null), json!(null), json!(9)],
        )],
    );
    assert_eq!(out, json!({ "xs": [1, null, null, 9] }));
}

#[test]
fn unresolvable_splice_target_errors() {
    // `p` must resolve to an array (delta/index.ts:1392-1394).
    assert!(matches!(
        apply_err(
            Some(&json!({ "xs": 1 })),
            &[splice(&[k("xs")], 0, 0, vec![])]
        ),
        DeltaError::UnresolvablePath { .. }
    ));
}

// ─── op_from_json: assertValidOp ────────────────────────────────────────────

#[test]
fn from_json_accepts_decoded_ops() {
    // delta.test.ts:1014-1027.
    for (tuple, op) in [
        (json!(["r", { "a": 1 }]), replace(json!({ "a": 1 }))),
        (json!(["s", ["a"], 1]), set(&[k("a")], json!(1))),
        (json!(["d", ["a"]]), delete(&[k("a")])),
        (json!(["a", ["a"], "x"]), append(&[k("a")], "x")),
        (json!(["t", ["a"], 2]), truncate(&[k("a")], 2)),
        (
            json!(["p", ["a"], 0, 0, []]),
            splice(&[k("a")], 0, 0, vec![]),
        ),
    ] {
        assert_eq!(op_from_json(&tuple).expect("valid decoded op"), op);
    }
}

#[test]
fn from_json_rejects_wire_only_forms() {
    // Each vocabulary gets the validator that matches it
    // (delta.test.ts:1028-1039): decoded-op validation must reject ids and
    // short forms.
    for wire in [
        json!(["s", 1]),
        json!(["d"]),
        json!(["a", "x"]),
        json!(["t", 2]),
        json!(["p", 0, 0, []]),
        json!(["#", 0, ["a"]]),
        json!(["s", 0, 1]),
    ] {
        assert!(
            op_from_json(&wire).is_err(),
            "wire form must be rejected: {wire}"
        );
    }
}

#[test]
fn from_json_rejects_unknown_verbs_and_non_tuples() {
    // Silently skipping an unknown verb is how a newer producer's op vanishes
    // (delta/index.ts:1245-1247); delta.test.ts:964-979.
    assert!(op_from_json(&json!(["ZZZ", ["a"], 9])).is_err());
    assert!(op_from_json(&json!({ "op": "s" })).is_err());
    assert!(op_from_json(&json!(null)).is_err());
    assert!(op_from_json(&json!([])).is_err());
}

#[test]
fn from_json_rejects_bad_arity_and_payload_shapes() {
    assert!(op_from_json(&json!(["r"])).is_err());
    assert!(op_from_json(&json!(["r", 1, 2])).is_err());
    assert!(op_from_json(&json!(["s", ["a"]])).is_err());
    assert!(op_from_json(&json!(["a", ["a"], 9])).is_err());
    assert!(op_from_json(&json!(["t", ["a"], -1])).is_err());
    assert!(op_from_json(&json!(["t", ["a"], 1.5])).is_err());
    assert!(op_from_json(&json!(["p", ["a"], -1, 0, []])).is_err());
    // "rejects non-array splice items" (delta.test.ts:968-971): unvalidated,
    // this spreads a string into the array.
    assert!(op_from_json(&json!(["p", ["xs"], 0, 0, "not-an-array"])).is_err());
    assert!(op_from_json(&json!(["s", "a", 9])).is_err());
    assert!(op_from_json(&json!(["d", ["__proto__", "w"]])).is_err());
}

#[test]
fn from_json_parses_path_number_segments() {
    let op = op_from_json(&json!(["s", ["xs", 1], 9])).expect("integer index");
    assert_eq!(op, set(&[k("xs"), i(1)], json!(9)));
    // Fractional or negative path indices are unsafe segments.
    assert!(op_from_json(&json!(["s", ["xs", 1.5], 9])).is_err());
    assert!(op_from_json(&json!(["s", ["xs", -1], 9])).is_err());
}

#[test]
fn op_to_json_is_the_inverse_of_from_json() {
    for op in [
        replace(json!({ "a": [1, null, true] })),
        set(&[k("a"), i(2)], json!({ "b": "c" })),
        delete(&[k("a")]),
        append(&[k("a")], "text"),
        truncate(&[k("a")], 3),
        splice(&[], 1, 2, vec![json!(null), json!(false)]),
    ] {
        let tuple = op_to_json(&op);
        assert_eq!(op_from_json(&tuple).expect("round trip"), op);
    }
}

// ─── isBase ─────────────────────────────────────────────────────────────────

#[test]
fn is_base_checks_only_the_first_op() {
    // delta/index.ts:66-70: flush guarantees r is at index 0 or absent, so
    // this is exact rather than a heuristic.
    assert!(is_base(&[replace(json!(1))]));
    assert!(is_base(&[replace(json!(1)), set(&[k("a")], json!(2))]));
    assert!(!is_base(&[]));
    assert!(!is_base(&[set(&[k("a")], json!(2))]));
    assert!(!is_base(&[set(&[k("a")], json!(2)), replace(json!(1))]));
}

// ─── overlap and UTF-16 helpers ─────────────────────────────────────────────

#[test]
fn overlap_finds_an_overlap_shorter_than_the_long_probe() {
    // delta.test.ts:20-23.
    assert_eq!(overlap("abcdefgh", "defghxyz", 65_536), 5);
}

#[test]
fn overlap_honors_a_disabled_scan() {
    // delta.test.ts:25-27.
    assert_eq!(overlap("abcdef", "defghi", 0), 0);
}

#[test]
fn overlap_is_exact() {
    // "Always correct: the returned n satisfies a.slice(a.length - n) ===
    // b.slice(0, n)" (delta/index.ts:75-80).
    assert_eq!(overlap("xyz", "abc", 65_536), 0);
    assert_eq!(overlap("abc", "abc", 65_536), 3);
    // Rolling window: "abcdef" -> "defabc" shares "def".
    assert_eq!(overlap("abcdef", "defabc", 65_536), 3);
    // Bounded scan limits the suffix considered.
    assert_eq!(overlap("abcdef", "defabc", 3), 3);
    assert_eq!(overlap("abcdef", "defabc", 2), 0);
    assert_eq!(overlap("", "abc", 65_536), 0);
    assert_eq!(overlap("abc", "", 65_536), 0);
}

#[test]
fn overlap_counts_utf16_units() {
    // "😀ab" is 4 UTF-16 units; its suffix "ab" is a prefix of "abc".
    assert_eq!(overlap("😀ab", "abc", 65_536), 2);
}

#[test]
fn utf16_helpers_match_js_string_length_and_slice() {
    assert_eq!(utf16_len("ab😀cd"), 6);
    assert_eq!(utf16_len(""), 0);
    assert_eq!(slice_utf16_from("abcdef", 2), "cdef");
    assert_eq!(slice_utf16_from("abc", 3), "");
    assert_eq!(slice_utf16_from("abc", 99), "");
    // A start inside a surrogate pair slices at the next char boundary;
    // unit 4 of "ab😀cd" (units a=0, b=1, surrogate pair=2-3) is "cd".
    assert_eq!(slice_utf16_from("ab😀cd", 3), "cd");
    assert_eq!(slice_utf16_from("ab😀cd", 4), "cd");
    assert_eq!(slice_utf16_from("ab😀cd", 2), "😀cd");
}

// ─── tracker: lifecycle ─────────────────────────────────────────────────────

#[test]
fn the_first_flush_is_always_a_base_batch() {
    // delta.test.ts:532-542: a consumer starts with nothing, so the stream
    // opens with a replacement carrying mutations made before it.
    let mut t = track(json!({ "x": 0 }));
    t.state_mut()["x"] = json!(100);
    assert!(t.is_dirty());
    let ops = t.flush();
    assert!(is_base(&ops));
    assert_eq!(ops, vec![replace(json!({ "x": 100 }))]);
}

#[test]
fn an_untouched_tracker_still_owes_its_base_batch() {
    // delta.test.ts:551-559.
    let mut t = track(json!({ "x": 0 }));
    assert!(t.is_dirty());
    assert_eq!(t.flush(), vec![replace(json!({ "x": 0 }))]);
    assert!(!t.is_dirty());
    assert!(t.flush().is_empty());
}

#[test]
fn the_first_flush_is_followed_by_deltas() {
    // delta.test.ts:544-549.
    let mut t = track(json!({ "x": 0, "pad": "p" }));
    t.flush();
    t.state_mut()["x"] = json!(1);
    assert_eq!(t.flush(), vec![set(&[k("x")], json!(1))]);
}

#[test]
fn discard_accepts_pending_changes_without_publishing() {
    // delta.test.ts:561-571.
    let mut t = track(json!({ "x": 0, "y": 0 }));
    t.flush();
    t.state_mut()["x"] = json!(1);
    assert!(t.is_dirty());
    t.discard();
    assert!(!t.is_dirty());
    assert!(t.flush().is_empty());
    t.state_mut()["y"] = json!(1);
    assert_eq!(t.flush(), vec![set(&[k("y")], json!(1))]);
}

#[test]
fn rebase_forces_one_base_batch_without_changing_the_value() {
    // delta.test.ts:510-529: this is the checkpoint; recovery replays from the
    // last base batch.
    let mut t = track(json!({ "p": 1, "pad": "p" }));
    t.flush();
    t.rebase();
    let ops = t.flush();
    assert!(is_base(&ops));
    assert_eq!(apply_ok(None, &ops), *t.state());
    // Applies once, not to every later flush.
    t.flush();
    t.state_mut()["p"] = json!(2);
    assert_eq!(t.flush(), vec![set(&[k("p")], json!(2))]);
}

#[test]
fn discard_before_the_first_flush_still_owes_the_base() {
    // clearPending leaves forceBase alone (delta/index.ts:338-347,1155-1157).
    let mut t = track(json!({ "x": 0 }));
    t.state_mut()["x"] = json!(1);
    t.discard();
    assert!(t.is_dirty());
    assert_eq!(t.flush(), vec![replace(json!({ "x": 1 }))]);
}

#[test]
fn set_value_replaces_the_whole_value_as_a_base_batch() {
    // `tracker.state = next` (delta.test.ts:460-499).
    let mut t = track(json!({ "p": 1, "q": "pad" }));
    t.flush();
    t.state_mut()["p"] = json!(2);
    t.set_value(json!({ "r": 9, "s": "new" }));
    let ops = t.flush();
    assert_eq!(ops, vec![replace(json!({ "r": 9, "s": "new" }))]);
    assert!(is_base(&ops));
    assert_eq!(apply_ok(Some(&json!({})), &ops), *t.state());
    // The new value is tracked.
    t.state_mut()["r"] = json!(10);
    assert_eq!(t.flush(), vec![set(&[k("r")], json!(10))]);
}

#[test]
fn flush_is_empty_when_nothing_is_pending() {
    let mut t = track(json!({ "x": 0 }));
    t.flush();
    assert!(t.flush().is_empty());
}

// ─── tracker: intent (shapes shared with upstream's flush-side diff) ────────

#[test]
fn records_an_append_not_a_replacement() {
    // delta.test.ts:31-39.
    let mut t = track(json!({ "s": "", "pad": "p".repeat(400) }));
    t.flush();
    t.state_mut()["s"] = json!("ab");
    t.state_mut()["s"] = json!("abcd");
    let ops = t.flush();
    assert_eq!(ops, vec![append(&[k("s")], "abcd")]);
    assert_eq!(
        apply_ok(Some(&json!({ "s": "", "pad": "p".repeat(400) })), &ops),
        *t.state()
    );
}

#[test]
fn recovers_truncate_append_from_a_rolling_window() {
    // delta.test.ts:41-52: the case every effect-recording library degrades to
    // a whole-value set.
    let initial = json!({ "s": "abcdefgh", "pad": "p".repeat(400) });
    let mut t = track(initial.clone());
    t.flush();
    let next = format!("{}xyz", &initial["s"].as_str().unwrap()[3..]);
    t.state_mut()["s"] = json!(next);
    let ops = t.flush();
    assert_eq!(ops, vec![truncate(&[k("s")], 3), append(&[k("s")], "xyz")]);
    assert_eq!(apply_ok(Some(&initial), &ops), *t.state());
}

#[test]
fn records_array_intent_as_one_splice() {
    // delta.test.ts:63-68.
    let mut t = track(json!({ "xs": [1, 2], "pad": "p".repeat(400) }));
    t.flush();
    t.state_mut()["xs"]
        .as_array_mut()
        .expect("array")
        .push(json!(3));
    assert_eq!(t.flush(), vec![splice(&[k("xs")], 2, 0, vec![json!(3)])]);
}

#[test]
fn collapses_many_pushes_into_one_append() {
    // delta.test.ts:403-408.
    let mut t = track(json!({ "xs": [1] }));
    t.flush();
    let array = t.state_mut()["xs"].as_array_mut().expect("array");
    for value in 2..=100 {
        array.push(json!(value));
    }
    let items: Vec<JsonValue> = (2..=100).map(|v| json!(v)).collect();
    assert_eq!(t.flush(), vec![splice(&[k("xs")], 1, 0, items)]);
}

#[test]
fn collapses_repeated_writes_to_one_field() {
    // delta.test.ts:660-667.
    let mut t = track(json!({ "a": { "b": 1 }, "x": 1, "y": 2 }));
    t.flush();
    t.state_mut()["x"] = json!(1);
    t.state_mut()["x"] = json!(2);
    t.state_mut()["x"] = json!(3);
    assert_eq!(t.flush(), vec![set(&[k("x")], json!(3))]);
}

#[test]
fn drops_a_write_superseded_by_a_later_non_adjacent_one() {
    // delta.test.ts:669-681 — as a set of ops; ordering of unrelated paths is
    // an implementation detail (BTreeMap key order here).
    let mut t = track(json!({ "a": { "b": 1 }, "x": 1, "y": 2 }));
    t.flush();
    t.state_mut()["x"] = json!(10);
    t.state_mut()["y"] = json!(20);
    t.state_mut()["x"] = json!(30);
    assert_eq!(
        sorted(t.flush()),
        sorted(vec![set(&[k("y")], json!(20)), set(&[k("x")], json!(30))])
    );
}

#[test]
fn collapses_set_then_delete_to_the_delete() {
    // delta.test.ts:704-711.
    let mut t = track(json!({ "a": { "b": 1 }, "x": 1, "y": 2 }));
    t.flush();
    t.state_mut()["x"] = json!(5);
    t.state_mut().as_object_mut().expect("object").remove("x");
    assert_eq!(t.flush(), vec![delete(&[k("x")])]);
}

#[test]
fn collapses_delete_then_set_to_the_final_value() {
    // delta.test.ts:713-721.
    let mut t = track(json!({ "a": { "b": 1 }, "x": 1, "y": 2 }));
    t.flush();
    t.state_mut().as_object_mut().expect("object").remove("x");
    t.state_mut()["x"] = json!(5);
    assert_eq!(t.flush(), vec![set(&[k("x")], json!(5))]);
}

#[test]
fn folds_a_child_write_into_its_pending_parent_replacement() {
    // delta.test.ts:695-702 — the diff produces the per-key set directly.
    let mut t = track(json!({ "a": { "b": 1 }, "x": 1, "y": 2 }));
    t.flush();
    t.state_mut()["a"] = json!({ "b": 1 });
    t.state_mut()["a"]["b"] = json!(7);
    assert_eq!(t.flush(), vec![set(&[k("a"), k("b")], json!(7))]);
}

#[test]
fn invalidates_a_pending_child_when_its_parent_is_overwritten() {
    // delta.test.ts:167-174 (convergence form).
    let initial = json!({ "a": { "x": 1 } });
    let mut t = track(initial.clone());
    t.flush();
    t.state_mut()["a"]["b"] = json!(99);
    t.state_mut()["a"] = json!({ "c": 2 });
    let ops = t.flush();
    assert_eq!(apply_ok(Some(&initial), &ops), *t.state());
    assert_eq!(*t.state(), json!({ "a": { "c": 2 } }));
}

#[test]
fn emits_nothing_when_a_replacement_is_deeply_equal() {
    // delta.test.ts:158-165.
    let mut t = track(json!({ "value": { "nested": [1, { "text": "same" }] } }));
    t.flush();
    t.state_mut()["value"] = json!({ "nested": [1, { "text": "same" }] });
    assert!(t.is_dirty());
    assert!(t.flush().is_empty());
    assert!(!t.is_dirty());
}

#[test]
fn deep_diffs_reassigned_objects() {
    // delta.test.ts:104-113.
    let initial = json!({ "message": { "content": [{ "text": "hello" }], "count": 0 } });
    let mut t = track(initial.clone());
    t.flush();
    t.state_mut()["message"] = json!({ "content": [{ "text": "hello world" }], "count": 1 });
    let ops = t.flush();
    assert_eq!(
        ops,
        vec![
            append(&[k("message"), k("content"), i(0), k("text")], " world"),
            set(&[k("message"), k("count")], json!(1)),
        ]
    );
    assert_eq!(apply_ok(Some(&initial), &ops), *t.state());
}

#[test]
fn deep_diffs_retained_array_edits_combined_with_an_append() {
    // delta.test.ts:115-126.
    let initial = json!({ "view": { "messages": [{ "text": "a" }, { "text": "b" }] } });
    let mut t = track(initial.clone());
    t.flush();
    t.state_mut()["view"] =
        json!({ "messages": [{ "text": "ax" }, { "text": "b" }, { "text": "c" }] });
    let ops = t.flush();
    assert_eq!(
        ops,
        vec![
            append(&[k("view"), k("messages"), i(0), k("text")], "x"),
            splice(
                &[k("view"), k("messages")],
                2,
                0,
                vec![json!({ "text": "c" })]
            ),
        ]
    );
    assert_eq!(apply_ok(Some(&initial), &ops), *t.state());
}

#[test]
fn normalises_undefined_to_delete_for_optional_properties() {
    // delta.test.ts:77-89, using absence and delete for optional properties.
    let mut t = track(json!({ "foo": 1 }));
    let mut replica: Option<JsonValue> = None;
    fold(&mut replica, &t.flush());
    t.state_mut()["something"] = json!("enabled");
    fold(&mut replica, &t.flush());
    assert_eq!(replica, Some(json!({ "foo": 1, "something": "enabled" })));
    t.state_mut()
        .as_object_mut()
        .expect("object")
        .remove("something");
    let ops = t.flush();
    assert_eq!(ops, vec![delete(&[k("something")])]);
    fold(&mut replica, &ops);
    assert_eq!(replica, Some(json!({ "foo": 1 })));
}

#[test]
fn a_root_array_push_splices_the_empty_path() {
    // delta.test.ts:178-185: `p` may address a root array.
    let mut t = track(json!([1, 2, 3, "pad"]));
    t.flush();
    t.state_mut().as_array_mut().expect("array").push(json!(4));
    let ops = t.flush();
    assert_eq!(ops, vec![splice(&[], 4, 0, vec![json!(4)])]);
    assert_eq!(apply_ok(Some(&json!([1, 2, 3, "pad"])), &ops), *t.state());
}

#[test]
fn replacing_the_root_value_with_an_empty_array_is_a_replacement() {
    // delta.test.ts:227-234 (length = 0 on the root).
    let mut t = track(json!([1, 2, 3]));
    t.flush();
    t.set_value(json!([]));
    let ops = t.flush();
    assert_eq!(ops, vec![replace(json!([]))]);
    assert_eq!(apply_ok(Some(&json!([1, 2, 3])), &ops), json!([]));
}

#[test]
fn a_nested_splice_all_is_a_set_not_a_replacement() {
    // delta.test.ts:196-201.
    let initial = json!({ "xs": [1, 2, 3], "pad": "p".repeat(400) });
    let mut t = track(initial.clone());
    t.flush();
    *t.state_mut() = json!({ "xs": [9], "pad": "p".repeat(400) });
    let ops = t.flush();
    assert_eq!(
        apply_ok(Some(&initial), &ops),
        json!({ "xs": [9], "pad": "p".repeat(400) })
    );
    // A whole-subtree set, not a root replacement.
    assert!(!is_base(&ops));
}

#[test]
fn root_splice_replacement_collapses_to_a_replacement() {
    // delta.test.ts:187-194: a splice covering the whole root is normalised to
    // a replacement.
    let mut t = track(json!([1, 2, 3]));
    t.flush();
    t.set_value(json!([9]));
    let ops = t.flush();
    assert_eq!(ops, vec![replace(json!([9]))]);
    assert!(is_base(&ops));
}

#[test]
fn a_partial_rewrite_keeps_its_ops() {
    // delta.test.ts:501-508.
    let mut t = track(json!({ "a": "x".repeat(200), "b": "y".repeat(200) }));
    t.flush();
    t.state_mut()["a"] = json!("p");
    let ops = t.flush();
    assert!(!is_base(&ops));
    assert_eq!(ops, vec![set(&[k("a")], json!("p"))]);
}

#[test]
fn disabled_overlap_scan_degrades_to_a_set() {
    // diffString with scan = 0: overlap returns 0, which emits a set —
    // larger, never wrong (delta/index.ts:88-93).
    let initial = json!({ "s": "abcdefgh" });
    let mut t = Tracker::with_options(
        initial.clone(),
        TrackerOptions {
            max_overlap_scan: 0,
        },
    );
    t.flush();
    t.state_mut()["s"] = json!("defghxyz");
    let ops = t.flush();
    assert_eq!(ops, vec![set(&[k("s")], json!("defghxyz"))]);
    assert_eq!(apply_ok(Some(&initial), &ops), *t.state());
}

#[test]
fn string_diff_counts_utf16_units_across_astral_content() {
    // The window "😀ab" -> "bc": one surviving unit ("b"), so the truncate
    // count is 3 units even though only 2 chars are removed.
    let initial = json!({ "msg": "😀ab" });
    let mut t = track(initial.clone());
    t.flush();
    t.state_mut()["msg"] = json!("bc");
    let ops = t.flush();
    assert_eq!(
        ops,
        vec![truncate(&[k("msg")], 3), append(&[k("msg")], "c")]
    );
    assert_eq!(apply_ok(Some(&initial), &ops), *t.state());
}

#[test]
fn string_append_after_a_prefix_growth() {
    // diffString: after.startsWith(before) emits a single append of the
    // suffix (delta/index.ts:211-214).
    let initial = json!({ "s": "hello" });
    let mut t = track(initial.clone());
    t.flush();
    t.state_mut()["s"] = json!("hello world");
    let ops = t.flush();
    assert_eq!(ops, vec![append(&[k("s")], " world")]);
    assert_eq!(apply_ok(Some(&initial), &ops), *t.state());
}

// ─── tracker: convergence ───────────────────────────────────────────────────

#[test]
fn folds_a_whole_stream_without_a_base_batch_branch() {
    // delta.test.ts:629-646: apply handles r by replacing and tolerates an
    // undefined target.
    let mut t = track(json!({ "x": 0, "l": [] }));
    let mut replica: Option<JsonValue> = None;
    t.state_mut()["x"] = json!(100);
    t.state_mut()["l"]
        .as_array_mut()
        .expect("array")
        .push(json!("xyz"));
    fold(&mut replica, &t.flush());
    t.state_mut()["x"] = json!(101);
    fold(&mut replica, &t.flush());
    assert_eq!(replica, Some(t.state().clone()));
}

#[test]
fn round_trips_interleaved_writes() {
    // delta.test.ts:91-102.
    let initial = json!({ "out": "x".repeat(500), "total": 0 });
    let mut t = track(initial.clone());
    t.flush();
    for count in 0..1_000 {
        let current = t.state()["out"].as_str().expect("string").to_owned();
        let next = format!("{}{count:010}", &current[10..]);
        t.state_mut()["out"] = json!(next);
        let total = t.state()["total"].as_i64().expect("number");
        t.state_mut()["total"] = json!(total + 10);
    }
    let ops = t.flush();
    assert!(ops.len() <= 3, "expected at most 3 ops, got {ops:?}");
    assert_eq!(apply_ok(Some(&initial), &ops), *t.state());
}

#[test]
fn updates_an_anchored_string_through_a_later_container_replacement() {
    // delta.test.ts:128-135.
    let initial = json!({ "xs": [{ "k": "a" }] });
    let mut t = track(initial.clone());
    t.flush();
    t.state_mut()["xs"][0]["k"] = json!("ab");
    t.state_mut()["xs"] = json!([{ "k": "abc" }, { "k": "z" }]);
    let ops = t.flush();
    assert_eq!(apply_ok(Some(&initial), &ops), *t.state());
}

#[test]
fn does_not_fold_a_child_path_across_a_parent_splice() {
    // delta.test.ts:245-253.
    let initial = json!({ "xs": ["ab", "cd"] });
    let mut t = track(initial.clone());
    t.flush();
    let array = t.state_mut()["xs"].as_array_mut().expect("array");
    array[0] = json!("abx");
    array.remove(0);
    array[0] = json!("cdy");
    let ops = t.flush();
    assert_eq!(apply_ok(Some(&initial), &ops), *t.state());
}

#[test]
fn preserves_element_operations_across_middle_array_insertions() {
    // delta.test.ts:285-293.
    let initial = json!({ "xs": ["a", "b"] });
    let mut t = track(initial.clone());
    t.flush();
    let array = t.state_mut()["xs"].as_array_mut().expect("array");
    array[1] = json!("bx");
    array.insert(1, json!("inserted"));
    array[1] = json!("changed");
    let ops = t.flush();
    assert_eq!(apply_ok(Some(&initial), &ops), *t.state());
}

#[test]
fn keeps_direct_tail_writes_and_length_growth_in_append_mode() {
    // delta.test.ts:410-417 (convergence form).
    let initial = json!({ "xs": [1] });
    let mut t = track(initial.clone());
    t.flush();
    {
        let array = t.state_mut()["xs"].as_array_mut().expect("array");
        array.push(json!({ "value": 2 }));
        array[1] = json!({ "value": 3 });
        array.push(json!(null));
        array.push(json!(null));
    }
    let ops = t.flush();
    assert_eq!(
        apply_ok(Some(&initial), &ops),
        json!({ "xs": [1, { "value": 3 }, null, null] })
    );
}

#[test]
fn keeps_mutator_chaining_tracked() {
    // delta.test.ts:379-385 (sort + push, convergence form).
    let initial = json!({ "xs": [3, 1, 2] });
    let mut t = track(initial.clone());
    t.flush();
    {
        let array = t.state_mut()["xs"].as_array_mut().expect("array");
        array.sort_by_key(|a| a.to_string());
        array.push(json!(4));
    }
    let ops = t.flush();
    assert_eq!(apply_ok(Some(&initial), &ops), *t.state());
    assert_eq!(*t.state(), json!({ "xs": [1, 2, 3, 4] }));
}

#[test]
fn drops_everything_before_a_root_replacement() {
    // delta.test.ts:795-802.
    let mut t = track(json!([1, 2]));
    t.flush();
    t.state_mut().as_array_mut().expect("array").push(json!(3));
    t.set_value(json!([7]));
    assert_eq!(t.flush(), vec![replace(json!([7]))]);
}

#[test]
fn keeps_the_prefix_when_the_replacement_is_nested() {
    // delta.test.ts:804-814: `s` on a subtree is not a root replacement, so
    // earlier ops stay live.
    let initial = json!({ "a": 1, "xs": [1, 2, 3], "pad": "p".repeat(400) });
    let mut t = track(initial.clone());
    t.flush();
    t.state_mut()["a"] = json!(2);
    t.state_mut()["xs"] = json!([9]);
    let ops = t.flush();
    assert!(!is_base(&ops));
    assert_eq!(apply_ok(Some(&initial), &ops), *t.state());
    assert_eq!(
        *t.state(),
        json!({ "a": 2, "xs": [9], "pad": "p".repeat(400) })
    );
}

#[test]
fn a_value_that_merely_looks_like_an_op_is_just_a_value() {
    // delta.test.ts:1005-1010.
    let mut t = track(json!({ "pad": "p".repeat(400) }));
    t.flush();
    t.state_mut()["x"] = json!(["r", { "evil": true }]);
    assert_eq!(
        t.flush(),
        vec![set(&[k("x")], json!(["r", { "evil": true }]))]
    );
}

#[test]
fn reserved_value_keys_are_preserved_by_the_tracker() {
    // delta.test.ts:892-901: reserved names are legal as VALUE keys; reading
    // and serialising them works, mutating through the key is what is
    // blocked (upstream blocks it in the proxy; the port has no proxy, so
    // only the publish path exists and the value round-trips).
    let mut t = track(json!({ "value": { "__proto__": { "z": 1 } } }));
    let ops = t.flush();
    let out = apply_ok(None, &ops);
    assert_eq!(out["value"]["__proto__"], json!({ "z": 1 }));
}

#[test]
fn replaces_the_parent_when_an_assigned_object_removes_a_reserved_value_key() {
    // delta.test.ts:881-890.
    let initial = json!({ "value": { "constructor": { "label": "data" }, "x": 1 } });
    let mut t = track(initial.clone());
    t.flush();
    t.state_mut()["value"] = json!({ "x": 2 });
    let ops = t.flush();
    assert_eq!(apply_ok(Some(&initial), &ops), *t.state());
}

#[test]
fn round_trips_repeated_nested_splices() {
    // delta.test.ts:236-243.
    let initial = json!({ "xs": [1] });
    let mut t = track(initial.clone());
    t.flush();
    let array = t.state_mut()["xs"].as_array_mut().expect("array");
    for count in 2..=100 {
        array.push(json!(count));
    }
    let ops = t.flush();
    assert_eq!(apply_ok(Some(&initial), &ops), *t.state());
}

// ─── tracker: property (seeded, deterministic) ──────────────────────────────

/// Deterministic LCG, mirroring delta.test.ts:1187-1190.
struct Lcg(u32);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        self.0
    }

    fn below(&mut self, bound: u32) -> u32 {
        self.next() % bound
    }
}

#[test]
fn property_converges_across_mixed_writes_replacements_and_array_mutations() {
    // delta.test.ts:1183-1253 (convergence form).
    let mut rng = Lcg(0x5eed_1234);
    for _round in 0..100 {
        let initial = json!({
            "rows": [
                { "text": "a", "count": 0 },
                { "text": "b", "count": 0 },
            ],
            "meta": { "revision": 0 },
        });
        let mut t = track(initial.clone());
        let mut replica: Option<JsonValue> = None;
        fold(&mut replica, &t.flush());
        for step in 0..60 {
            {
                let state = t.state_mut();
                let rows_len = state["rows"].as_array().expect("rows").len();
                match rng.below(10) {
                    0 => {
                        if rows_len > 0 {
                            let at = rng.below(rows_len as u32) as usize;
                            let suffix = char::from_u32(97 + rng.below(26)).expect("ascii");
                            let current =
                                state["rows"][at]["text"].as_str().expect("text").to_owned();
                            state["rows"][at]["text"] = json!(format!("{current}{suffix}"));
                        }
                    }
                    1 => {
                        if rows_len > 0 {
                            let at = rng.below(rows_len as u32) as usize;
                            let count = state["rows"][at]["count"].as_i64().expect("count");
                            state["rows"][at]["count"] = json!(count + 1);
                        }
                    }
                    2 => {
                        let tail = json!({ "text": format!("tail-{step}"), "count": step });
                        state["rows"].as_array_mut().expect("rows").push(tail);
                    }
                    3 => {
                        if rows_len > 0 {
                            state["rows"].as_array_mut().expect("rows").remove(0);
                        }
                    }
                    4 => {
                        let head = json!({ "text": format!("head-{step}"), "count": step });
                        state["rows"].as_array_mut().expect("rows").insert(0, head);
                    }
                    5 => {
                        if rows_len > 0 {
                            state["rows"].as_array_mut().expect("rows").remove(0);
                        }
                    }
                    6 => {
                        let at = rng.below(rows_len as u32 + 1) as usize;
                        // JS Array.prototype.splice clamps a removal past the
                        // tail; clamp here for the same shape.
                        let remove = usize::from(rows_len > 0 && rng.below(2) == 0);
                        let end = (at + remove).min(rows_len);
                        let mid = json!({ "text": format!("mid-{step}"), "count": step });
                        state["rows"]
                            .as_array_mut()
                            .expect("rows")
                            .splice(at..end, [mid]);
                    }
                    7 => {
                        let replacement = state["rows"].clone();
                        state["rows"] = replacement;
                    }
                    8 => {
                        let revision = state["meta"]["revision"].as_i64().expect("revision");
                        state["meta"] = json!({ "revision": revision + 1 });
                    }
                    _ => {
                        if rows_len > 1 {
                            state["rows"].as_array_mut().expect("rows").reverse();
                        }
                    }
                }
                let rows_len = state["rows"].as_array().expect("rows").len();
                if rows_len > 12 {
                    let excess = rows_len - 12;
                    state["rows"].as_array_mut().expect("rows").drain(0..excess);
                }
            }
            if rng.below(5) == 0 {
                fold(&mut replica, &t.flush());
                assert_eq!(
                    replica.as_ref(),
                    Some(t.state()),
                    "mid-stream replica diverged"
                );
            }
        }
        fold(&mut replica, &t.flush());
        assert_eq!(replica.as_ref(), Some(t.state()), "final replica diverged");
    }
}

#[test]
fn property_random_document_mutation_converges() {
    // Adapted from delta.test.ts:1255-1290 (random round-trip), seeded.
    let mut rng = Lcg(0x1234_abcd);
    let mut checked = 0;
    for _ in 0..300 {
        let base = if rng.below(2) == 0 {
            json!({ "a": 1, "b": [1, 2, null], "c": "s" })
        } else {
            json!({ "x": { "y": "z" }, "l": [true, 0] })
        };
        let mut t = track(base.clone());
        let mut replica: Option<JsonValue> = None;
        // The first flush is the base batch that hydrates the replica.
        fold(&mut replica, &t.flush());
        {
            let state = t.state_mut();
            let object = state.as_object_mut().expect("object");
            let keys: Vec<String> = object.keys().cloned().collect();
            // Delete a random subset, then set random leaves.
            for key in &keys {
                if rng.below(3) == 0 {
                    object.remove(key);
                }
            }
            if rng.below(2) == 0 {
                object.insert("added".to_owned(), json!(rng.below(5)));
            }
            if let Some(existing) = object.get_mut("l") {
                existing
                    .as_array_mut()
                    .expect("array")
                    .push(json!(rng.below(9)));
            }
        }
        fold(&mut replica, &t.flush());
        assert_eq!(replica.as_ref(), Some(t.state()));
        checked += 1;
    }
    assert!(checked > 200, "guard against silently checking nothing");
}

#[test]
fn bounds_long_structural_windows_by_converging() {
    // delta.test.ts:780-792 (convergence form; upstream additionally collapses
    // its internal log to a base batch, which the port does not need — its
    // flush is one diff regardless of window length).
    let initial = json!({ "xs": [{ "value": 0 }, { "value": 1 }] });
    let mut t = track(initial.clone());
    t.flush();
    for count in 0..5_000u32 {
        let array = t.state_mut()["xs"].as_array_mut().expect("array");
        array[0]["value"] = json!(count);
        array.remove(0);
        array.push(json!({ "value": count }));
    }
    let ops = t.flush();
    assert_eq!(apply_ok(Some(&initial), &ops), *t.state());
}

#[test]
fn pathologi_redundant_producer_converges_in_a_few_ops() {
    // delta.test.ts:767-778 (convergence + bounded batch form).
    let initial = json!({ "a": { "b": 1 }, "x": 1 });
    let mut t = track(initial.clone());
    t.flush();
    for count in 0..2000 {
        t.state_mut()["x"] = json!(count);
        t.state_mut()["a"]["b"] = json!(count);
    }
    t.state_mut()["a"] = json!({ "done": true });
    let ops = t.flush();
    assert!(ops.len() <= 3, "expected few ops, got {ops:?}");
    assert_eq!(apply_ok(Some(&initial), &ops), *t.state());
}

#[test]
fn json_order_delete_survives_legacy_delta_projection() {
    let input = json!({"drop":0,"z":1,"a":2,"nested":{"drop":0,"y":3,"b":4}});
    let before = input.to_string();
    let ops = [
        Op::Delete {
            path: vec![k("drop")],
        },
        Op::Delete {
            path: vec![k("nested"), k("drop")],
        },
    ];
    assert_eq!(
        apply_immutable(Some(&input), &ops).unwrap().to_string(),
        r#"{"z":1,"a":2,"nested":{"y":3,"b":4}}"#
    );
    assert_eq!(input.to_string(), before);
}
