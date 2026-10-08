//! Port of `packages/chord/src/delta/diff.ts` (upstream sha256
//! `4d1ee245aef0967154ff7177527dcf68606cde400f6cfcfe00cedd4c1e21986e`):
//! `diffRevisions`, the two-revision diff engine that computes a compact
//! operation batch between immutable JSON values.
//!
//! The engine anchors unchanged array entries by strict identity
//! (`===`) and falls back to structural equality (`equalJson`). Over owned
//! [`JsonValue`] trees strict identity is representable for primitives only
//! (JS aliasing of one container at two positions cannot exist); the port's
//! [`js_identical`] makes containers never identical, which is exactly the
//! upstream behavior for alias-free inputs — the only inputs
//! `diffRevisions` documents. The overflow set (`overflowedBatches`) is a
//! per-batch flag here: a batch is computed inside one `diff_revisions` call.

use std::collections::HashMap;

use super::{is_reserved, overlap, slice_utf16_from, utf16_len, JsonValue, Op, Seg};

const DEFAULT_OVERLAP_SCAN: usize = 65_536;
const MAX_DELTA_OPERATIONS: usize = 4_096;
const MAX_IDENTITY_CANDIDATES: usize = 200_000;
const MAX_SEMANTIC_CELLS: usize = 65_536;

/// The operation batch under construction plus the `overflowedBatches`
/// WeakSet entry (`diff.ts:6-15`): once the cap is reached the batch stops
/// accepting operations and `diff_revisions` collapses to a replacement.
#[derive(Default)]
struct Batch {
    operations: Vec<Op>,
    overflowed: bool,
}

impl Batch {
    /// `emitOperation` (`diff.ts:8-15`).
    fn emit(&mut self, operation: Op) {
        if self.overflowed {
            return;
        }
        if self.operations.len() >= MAX_DELTA_OPERATIONS {
            self.overflowed = true;
            return;
        }
        self.operations.push(operation);
    }
}

/// `emitSet` (`diff.ts:20-23`).
fn emit_set(batch: &mut Batch, path: &[Seg], value: &JsonValue) {
    if path.is_empty() {
        batch.emit(Op::Replace(value.clone()));
    } else {
        batch.emit(Op::Set {
            path: path.to_owned(),
            value: value.clone(),
        });
    }
}

/// Strict JS identity (`===`) over alias-free trees (`diff.ts:89`,
/// `diff.ts:165`): primitives compare by value (numbers as `f64`, where
/// `-0 === 0` holds and `NaN !== NaN`); containers are never identical
/// because an owned tree cannot alias one container at two positions.
pub(crate) fn js_identical(left: &JsonValue, right: &JsonValue) -> bool {
    match (left, right) {
        (JsonValue::Number(a), JsonValue::Number(b)) => {
            let (Some(a), Some(b)) = (a.as_f64(), b.as_f64()) else {
                return false;
            };
            // Number.isNaN on either side: JS `NaN !== NaN`.
            !(a.is_nan() || b.is_nan()) && a == b
        }
        (a, b) => a == b,
    }
}

/// `sameValue` (`diff.ts:89`): strict identity or full structural equality.
fn same_value(left: &JsonValue, right: &JsonValue) -> bool {
    if js_identical(left, right) {
        return true;
    }
    match (left, right) {
        (JsonValue::Array(_), JsonValue::Array(_))
        | (JsonValue::Object(_), JsonValue::Object(_)) => left == right,
        _ => false,
    }
}

/// `permutation` (`diff.ts:45-61`): a reorder mapping when `after` is a
/// permutation of `before`. The value→positions map has JS identity key
/// semantics, so container entries can never match across the two arrays;
/// the port keys primitives only.
fn permutation(before: &[JsonValue], after: &[JsonValue]) -> Option<Vec<usize>> {
    if before.len() != after.len() {
        return None;
    }
    let mut positions: HashMap<PrimitiveKey, (Vec<usize>, usize)> = HashMap::new();
    for (index, value) in before.iter().enumerate() {
        let Some(key) = PrimitiveKey::of(value) else {
            continue; // container: a key no lookup can ever hit
        };
        positions.entry(key).or_default().0.push(index);
    }
    let mut result = vec![0usize; after.len()];
    for (index, value) in after.iter().enumerate() {
        let key = PrimitiveKey::of(value)?;
        let entry = positions.get_mut(&key)?;
        if entry.1 == entry.0.len() {
            return None;
        }
        result[index] = entry.0[entry.1];
        entry.1 += 1;
    }
    Some(result)
}

/// Map key with JS SameValueZero semantics for primitives only.
#[derive(Clone, PartialEq, Eq, Hash)]
enum PrimitiveKey {
    Null,
    Bool(bool),
    Number(u64),
    Text(String),
}

impl PrimitiveKey {
    /// `None` for containers. `-0.0` and `0.0` share a key (SameValueZero);
    /// NaN keys equal themselves.
    fn of(value: &JsonValue) -> Option<PrimitiveKey> {
        match value {
            JsonValue::Null => Some(PrimitiveKey::Null),
            JsonValue::Bool(flag) => Some(PrimitiveKey::Bool(*flag)),
            JsonValue::Number(number) => {
                let raw = number.as_f64()?;
                let normalized = if raw == 0.0 { 0.0 } else { raw };
                Some(PrimitiveKey::Number(normalized.to_bits()))
            }
            JsonValue::String(text) => Some(PrimitiveKey::Text(text.clone())),
            JsonValue::Array(_) | JsonValue::Object(_) => None,
        }
    }
}

/// `emitString` (`diff.ts:63-76`).
fn emit_string(batch: &mut Batch, before: &str, after: &str, path: &[Seg]) {
    if before == after {
        return;
    }
    if after.len() > before.len() && after.starts_with(before) {
        batch.emit(Op::Append {
            path: path.to_owned(),
            text: after[before.len()..].to_owned(),
        });
        return;
    }
    let shared = overlap(before, after, DEFAULT_OVERLAP_SCAN);
    if shared == 0 {
        batch.emit(Op::Set {
            path: path.to_owned(),
            value: JsonValue::String(after.to_owned()),
        });
        return;
    }
    batch.emit(Op::Truncate {
        path: path.to_owned(),
        count: utf16_len(before) - shared,
    });
    if utf16_len(after) > shared {
        batch.emit(Op::Append {
            path: path.to_owned(),
            text: slice_utf16_from(after, shared).to_owned(),
        });
    }
}

/// One matched pair: `[before, after]` indices (`diff.ts:78`).
type ArrayMatch = (usize, usize);

/// `lcsMatches` (`diff.ts:91-122`): longest-common-subsequence match list,
/// or `None` when the cell budget is exceeded.
fn lcs_matches(
    before: &[JsonValue],
    after: &[JsonValue],
    max_cells: usize,
) -> Option<Vec<ArrayMatch>> {
    if before.is_empty() || after.is_empty() {
        return Some(Vec::new());
    }
    if before.len() * after.len() > max_cells {
        return None;
    }
    let width = after.len() + 1;
    let mut lengths = vec![0u32; (before.len() + 1) * width];
    for left in (0..before.len()).rev() {
        for right in (0..after.len()).rev() {
            let at = left * width + right;
            lengths[at] = if same_value(&before[left], &after[right]) {
                lengths[(left + 1) * width + right + 1] + 1
            } else {
                lengths[(left + 1) * width + right].max(lengths[left * width + right + 1])
            };
        }
    }
    let mut matches = Vec::new();
    let (mut left, mut right) = (0usize, 0usize);
    while left < before.len() && right < after.len() {
        if same_value(&before[left], &after[right])
            && lengths[left * width + right] == lengths[(left + 1) * width + right + 1] + 1
        {
            matches.push((left, right));
            left += 1;
            right += 1;
        } else if lengths[(left + 1) * width + right] >= lengths[left * width + right + 1] {
            left += 1;
        } else {
            right += 1;
        }
    }
    Some(matches)
}

/// `semanticallyAligned` (`diff.ts:124-138`), inlined into
/// [`lcs_matches`]: equal values, or containers of the same kind that share
/// an *identical* child. The identical-child probes use strict identity on
/// container children, which never fires over owned trees (JS aliasing is
/// unrepresentable), so the port's equality reduces to [`same_value`] —
/// the exact upstream behavior for alias-free inputs, which are the only
/// inputs `diffRevisions` documents.
///
/// `lowerBound` (`diff.ts:140-149`).
fn lower_bound(values: &[usize], value: usize) -> usize {
    values.partition_point(|at| *at < value)
}

/// `identitySubsequence` (`diff.ts:151-181`): a full-cover match by strict
/// identity when one side is strictly shorter.
fn identity_subsequence(
    before: &[JsonValue],
    before_start: usize,
    before_end: usize,
    after: &[JsonValue],
    after_start: usize,
    after_end: usize,
) -> Option<Vec<ArrayMatch>> {
    let before_count = before_end - before_start;
    let after_count = after_end - after_start;
    let mut matches = Vec::new();
    if after_count < before_count {
        let mut before_index = before_start;
        for (after_offset, after_value) in after[after_start..after_end].iter().enumerate() {
            let after_index = after_start + after_offset;
            while before_index < before_end && !js_identical(&before[before_index], after_value) {
                before_index += 1;
            }
            if before_index == before_end {
                return None;
            }
            matches.push((before_index, after_index));
            before_index += 1;
        }
        return Some(matches);
    }
    if before_count < after_count {
        let mut after_index = after_start;
        for (before_offset, before_value) in before[before_start..before_end].iter().enumerate() {
            let before_index = before_start + before_offset;
            while after_index < after_end && !js_identical(before_value, &after[after_index]) {
                after_index += 1;
            }
            if after_index == after_end {
                return None;
            }
            matches.push((before_index, after_index));
            after_index += 1;
        }
        return Some(matches);
    }
    None
}

/// `greedyIdentityAnchors` (`diff.ts:183-201`): first-fit anchors used once
/// the candidate budget would overflow.
fn greedy_identity_anchors(
    positions: &HashMap<PrimitiveKey, Vec<usize>>,
    after: &[JsonValue],
    after_start: usize,
    after_end: usize,
) -> Vec<ArrayMatch> {
    let mut matches = Vec::new();
    let mut previous: isize = -1;
    for (after_offset, after_value) in after[after_start..after_end].iter().enumerate() {
        let after_index = after_start + after_offset;
        let Some(key) = PrimitiveKey::of(after_value) else {
            continue;
        };
        let Some(candidates) = positions.get(&key) else {
            continue;
        };
        let at = lower_bound(candidates, (previous + 1) as usize);
        let Some(&before_index) = candidates.get(at) else {
            continue;
        };
        matches.push((before_index, after_index));
        previous = before_index as isize;
    }
    matches
}

/// `identityAnchors` (`diff.ts:203-251`): patience-style longest
/// strictly-increasing anchor subsequence over positions of identical
/// primitives (containers never anchor — identity keys).
fn identity_anchors(
    before: &[JsonValue],
    before_start: usize,
    before_end: usize,
    after: &[JsonValue],
    after_start: usize,
    after_end: usize,
) -> Vec<ArrayMatch> {
    let mut positions: HashMap<PrimitiveKey, Vec<usize>> = HashMap::new();
    for (offset, before_value) in before[before_start..before_end].iter().enumerate() {
        let index = before_start + offset;
        let Some(key) = PrimitiveKey::of(before_value) else {
            continue;
        };
        positions.entry(key).or_default().push(index);
    }
    let mut candidate_count = 0usize;
    for index in after_start..after_end {
        let Some(key) = PrimitiveKey::of(&after[index]) else {
            continue;
        };
        candidate_count += positions.get(&key).map_or(0, |at| at.len());
        if candidate_count > MAX_IDENTITY_CANDIDATES {
            return greedy_identity_anchors(&positions, after, after_start, after_end);
        }
    }
    if candidate_count == 0 {
        return Vec::new();
    }

    let mut candidates: Vec<MatchCandidate> = Vec::new();
    let mut tails: Vec<usize> = Vec::new();
    let mut tail_values: Vec<usize> = Vec::new();
    for (after_offset, after_value) in after[after_start..after_end].iter().enumerate() {
        let after_index = after_start + after_offset;
        let Some(key) = PrimitiveKey::of(after_value) else {
            continue;
        };
        let Some(before_positions) = positions.get(&key) else {
            continue;
        };
        for &before_index in before_positions.iter().rev() {
            let at = lower_bound(&tail_values, before_index);
            let candidate_index = candidates.len();
            candidates.push(MatchCandidate {
                before: before_index,
                after: after_index,
                previous: if at == 0 { usize::MAX } else { tails[at - 1] },
            });
            if at == tails.len() {
                tails.push(candidate_index);
                tail_values.push(before_index);
            } else {
                tails[at] = candidate_index;
                tail_values[at] = before_index;
            }
        }
    }
    let mut matches: Vec<ArrayMatch> = Vec::new();
    let mut candidate_index = tails.last().copied();
    while let Some(index) = candidate_index {
        let candidate = &candidates[index];
        matches.push((candidate.before, candidate.after));
        candidate_index = (candidate.previous != usize::MAX).then_some(candidate.previous);
    }
    matches.reverse();
    matches
}

struct MatchCandidate {
    before: usize,
    after: usize,
    previous: usize,
}

/// `processArrayMatches` (`diff.ts:253-282`).
#[allow(clippy::too_many_arguments)]
fn process_array_matches(
    batch: &mut Batch,
    before: &[JsonValue],
    after: &[JsonValue],
    path: &[Seg],
    before_start: usize,
    before_end: usize,
    after_start: usize,
    after_end: usize,
    output_start: usize,
    matches: &[ArrayMatch],
) {
    let (mut before_at, mut after_at, mut output_at) = (before_start, after_start, output_start);
    for &(before_match, after_match) in matches {
        if batch.overflowed {
            return;
        }
        diff_array_region(
            batch,
            before,
            after,
            path,
            before_at,
            before_match,
            after_at,
            after_match,
            output_at,
        );
        output_at += after_match - after_at;
        if !same_value(&before[before_match], &after[after_match]) {
            let mut sub = path.to_owned();
            sub.push(Seg::Index(output_at));
            diff_value(batch, &before[before_match], &after[after_match], &sub);
        }
        output_at += 1;
        before_at = before_match + 1;
        after_at = after_match + 1;
    }
    if !batch.overflowed {
        diff_array_region(
            batch, before, after, path, before_at, before_end, after_at, after_end, output_at,
        );
    }
}

/// `diffArrayRegion` (`diff.ts:284-401`).
#[allow(clippy::too_many_arguments)]
fn diff_array_region(
    batch: &mut Batch,
    before: &[JsonValue],
    after: &[JsonValue],
    path: &[Seg],
    mut before_start: usize,
    mut before_end: usize,
    mut after_start: usize,
    mut after_end: usize,
    mut output_start: usize,
) {
    if batch.overflowed {
        return;
    }
    while before_start < before_end
        && after_start < after_end
        && same_value(&before[before_start], &after[after_start])
    {
        before_start += 1;
        after_start += 1;
        output_start += 1;
    }
    while before_start < before_end
        && after_start < after_end
        && same_value(&before[before_end - 1], &after[after_end - 1])
    {
        before_end -= 1;
        after_end -= 1;
    }
    let before_count = before_end - before_start;
    let after_count = after_end - after_start;
    if before_count == 0 && after_count == 0 {
        return;
    }
    if before_count == 0 || after_count == 0 {
        batch.emit(Op::Splice {
            path: path.to_owned(),
            index: output_start,
            remove: before_count,
            items: after[after_start..after_end].to_vec(),
        });
        return;
    }

    if before_count == after_count {
        let positional: Vec<ArrayMatch> = (0..before_count)
            .filter(|offset| {
                same_value(&before[before_start + offset], &after[after_start + offset])
            })
            .map(|offset| (before_start + offset, after_start + offset))
            .collect();
        if !positional.is_empty() {
            process_array_matches(
                batch,
                before,
                after,
                path,
                before_start,
                before_end,
                after_start,
                after_end,
                output_start,
                &positional,
            );
            return;
        }
    }

    let subsequence = identity_subsequence(
        before,
        before_start,
        before_end,
        after,
        after_start,
        after_end,
    );
    if let Some(subsequence) = subsequence.filter(|at| !at.is_empty()) {
        process_array_matches(
            batch,
            before,
            after,
            path,
            before_start,
            before_end,
            after_start,
            after_end,
            output_start,
            &subsequence,
        );
        return;
    }

    let identity = identity_anchors(
        before,
        before_start,
        before_end,
        after,
        after_start,
        after_end,
    );
    if !identity.is_empty() {
        process_array_matches(
            batch,
            before,
            after,
            path,
            before_start,
            before_end,
            after_start,
            after_end,
            output_start,
            &identity,
        );
        return;
    }

    let semantic = lcs_matches(
        &before[before_start..before_end],
        &after[after_start..after_end],
        MAX_SEMANTIC_CELLS,
    );
    if let Some(semantic) = semantic.filter(|at| !at.is_empty()) {
        let absolute: Vec<ArrayMatch> = semantic
            .into_iter()
            .map(|(before_index, after_index)| {
                (before_start + before_index, after_start + after_index)
            })
            .collect();
        process_array_matches(
            batch,
            before,
            after,
            path,
            before_start,
            before_end,
            after_start,
            after_end,
            output_start,
            &absolute,
        );
        return;
    }

    if before_count == 1 && after_count == 1 {
        let mut sub = path.to_owned();
        sub.push(Seg::Index(output_start));
        diff_value(batch, &before[before_start], &after[after_start], &sub);
        return;
    }
    batch.emit(Op::Splice {
        path: path.to_owned(),
        index: output_start,
        remove: before_count,
        items: after[after_start..after_end].to_vec(),
    });
}

/// `diffArray` (`diff.ts:403-418`).
fn diff_array(batch: &mut Batch, before: &[JsonValue], after: &[JsonValue], path: &[Seg]) {
    if before == after {
        return;
    }
    if before.len() == after.len()
        && before.len() > 1
        && !same_value(&before[0], &after[0])
        && !same_value(&before[before.len() - 1], &after[after.len() - 1])
    {
        if let Some(order) = permutation(before, after) {
            batch.emit(Op::Reorder {
                path: path.to_owned(),
                permutation: order,
            });
            return;
        }
    }
    diff_array_region(
        batch,
        before,
        after,
        path,
        0,
        before.len(),
        0,
        after.len(),
        0,
    );
}

/// `diffObject` (`diff.ts:420-441`).
fn diff_object(
    batch: &mut Batch,
    before: &serde_json::Map<String, JsonValue>,
    after: &serde_json::Map<String, JsonValue>,
    path: &[Seg],
) {
    if before
        .keys()
        .chain(after.keys())
        .any(|key| is_reserved(key))
    {
        if before != after {
            emit_set(batch, path, &JsonValue::Object(after.clone()));
        }
        return;
    }
    for (key, value) in after {
        if batch.overflowed {
            return;
        }
        let mut sub = path.to_owned();
        sub.push(Seg::Key(key.clone()));
        match before.get(key) {
            Some(previous) => diff_value(batch, previous, value, &sub),
            None => emit_set(batch, &sub, value),
        }
    }
    for key in before.keys() {
        if batch.overflowed {
            return;
        }
        if !after.contains_key(key) {
            let mut sub = path.to_owned();
            sub.push(Seg::Key(key.clone()));
            batch.emit(Op::Delete { path: sub });
        }
    }
}

/// `diffValue` (`diff.ts:443-458`).
fn diff_value(batch: &mut Batch, before: &JsonValue, after: &JsonValue, path: &[Seg]) {
    if before == after || batch.overflowed {
        return;
    }
    if let (JsonValue::String(before), JsonValue::String(after)) = (before, after) {
        if !path.is_empty() {
            emit_string(batch, before, after, path);
            return;
        }
    }
    match (before, after) {
        (JsonValue::Array(before), JsonValue::Array(after)) => {
            diff_array(batch, before, after, path)
        }
        (JsonValue::Object(before), JsonValue::Object(after)) => {
            diff_object(batch, before, after, path)
        }
        _ => emit_set(batch, path, after),
    }
}

/// `jsonCost` (`diff.ts:460-476`): approximate wire size. Number sizes use
/// the JS `String(number)` length (`js_number_to_string`).
fn json_cost(value: &JsonValue) -> usize {
    match value {
        JsonValue::Null => 4,
        JsonValue::String(text) => utf16_len(text) + 2,
        JsonValue::Number(number) => js_number_to_string(number.as_f64().unwrap_or_default()).len(),
        JsonValue::Bool(flag) => {
            if *flag {
                4
            } else {
                5
            }
        }
        JsonValue::Array(items) => {
            let mut cost = 2;
            for (index, item) in items.iter().enumerate() {
                cost += json_cost(item) + usize::from(index > 0);
            }
            cost
        }
        JsonValue::Object(object) => {
            let mut cost = 2;
            for (index, (key, item)) in object.iter().enumerate() {
                cost += utf16_len(key) + 3 + json_cost(item) + usize::from(index > 0);
            }
            cost
        }
    }
}

/// `pathCost` (`diff.ts:478-485`).
fn path_cost(path: &[Seg]) -> usize {
    let mut cost = 2;
    for (index, segment) in path.iter().enumerate() {
        let size = match segment {
            Seg::Key(key) => utf16_len(key) + 2,
            Seg::Index(value) => value.to_string().len(),
        };
        cost += size + usize::from(index > 0);
    }
    cost
}

/// `operationCost` (`diff.ts:487-510`).
fn operation_cost(operation: &Op) -> usize {
    match operation {
        Op::Replace(value) => 6 + json_cost(value),
        Op::Set { path, value } => 7 + path_cost(path) + json_cost(value),
        Op::Delete { path } => 6 + path_cost(path),
        Op::Append { path, text } => 7 + path_cost(path) + utf16_len(text) + 2,
        Op::Truncate { path, count } => 7 + path_cost(path) + count.to_string().len(),
        Op::Splice {
            path,
            index,
            remove,
            items,
        } => {
            10 + path_cost(path)
                + index.to_string().len()
                + remove.to_string().len()
                + json_cost(&JsonValue::Array(items.clone()))
        }
        Op::Reorder { path, permutation } => {
            7 + path_cost(path)
                + json_cost(&JsonValue::Array(
                    permutation
                        .iter()
                        .map(|at| super::number_json(*at))
                        .collect(),
                ))
        }
    }
}

/// Compute a compact operation batch from two immutable JSON revisions.
/// Port of `diffRevisions` (`diff.ts:513-523`): capped batches collapse to a
/// full replacement, and a delta whose estimated wire cost exceeds the
/// snapshot cost collapses too.
pub fn diff_revisions(before: &JsonValue, after: &JsonValue) -> Vec<Op> {
    let mut batch = Batch::default();
    diff_value(&mut batch, before, after, &[]);
    if batch.overflowed {
        return vec![Op::Replace(after.clone())];
    }
    if batch.operations.is_empty() || matches!(batch.operations.first(), Some(Op::Replace(_))) {
        return batch.operations;
    }
    let mut delta_cost = 2usize;
    for operation in &batch.operations {
        delta_cost += operation_cost(operation) + 1;
    }
    if delta_cost < 65_536 {
        return batch.operations;
    }
    let snapshot_cost = json_cost(after) + 6;
    if delta_cost >= snapshot_cost {
        vec![Op::Replace(after.clone())]
    } else {
        batch.operations
    }
}

/// ECMAScript `Number::toString` (`js String(number)`) for cost arithmetic
/// and default sort keys. Handles the exponent thresholds the decimal form
/// uses; shortest-round-trip digits come from Rust's `{}`/`{:e}` float
/// formatting, which is the same shortest-digits algorithm V8 uses.
pub(crate) fn js_number_to_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value == 0.0 {
        return "0".to_owned();
    }
    if value < 0.0 {
        return format!("-{}", js_number_to_string(-value));
    }
    if value.is_infinite() {
        return "Infinity".to_owned();
    }
    // Shortest digits + decimal exponent: `d.dddde<exp>` from `{:e}`.
    let scientific = format!("{value:e}");
    let (mantissa, exponent) = scientific.split_once('e').expect("{:e} shape");
    let digits: String = mantissa.chars().filter(|c| c.is_ascii_digit()).collect();
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    let k = digits.len();
    let n: i64 = exponent.parse::<i64>().expect("{:e} exponent") + 1;
    if k as i64 <= n && n <= 21 {
        let mut out = digits.to_owned();
        for _ in 0..(n - k as i64) {
            out.push('0');
        }
        out
    } else if (0..=21).contains(&n) {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if (-6..=0).contains(&n) {
        format!("0.{}{}", "0".repeat((-n) as usize), digits)
    } else {
        let exp = n - 1;
        let sign = if exp < 0 { '-' } else { '+' };
        let tail = if k == 1 {
            String::new()
        } else {
            format!(".{}", &digits[1..])
        };
        format!("{}{}e{}{}", &digits[..1], tail, sign, exp.abs())
    }
}
