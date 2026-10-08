//! chord/delta — immutable revision tracking and operations over plain JSON.
//! Port of `packages/chord/src/delta/index.ts` at the upstream file split:
//!
//! | Upstream file | Port |
//! | --- | --- |
//! | `delta/index.ts` (core vocabulary, validators, applier, codec) | this file + [`codec`] |
//! | `delta/tracker.ts` | [`tracker`] |
//! | `delta/diff.ts` | [`diff`] |
//! | `delta/apply-immutable-trusted.ts` | [`apply_immutable_trusted`] |
//! | `delta/revision-validator.ts` | [`revision_validator`] |
//! | `delta/draft.ts` | the [`Draft`] alias |
//!
//! The re-exports below mirror the upstream `index.ts` export list exactly.
//!
//! # Vocabulary
//!
//! Upstream ops are JSON tuples (`delta/index.ts:30-40`); the port models
//! them as the [`Op`] enum with the same verb set — [`Op::Replace`] (`r`,
//! the only whole-value op), [`Op::Set`] (`s`, non-root), [`Op::Delete`]
//! (`d`, non-root), [`Op::Append`] (`a`, string append), [`Op::Truncate`]
//! (`t`, drop UTF-16 code units from the front), [`Op::Splice`] (`p`, array
//! splice; may target a root array) and [`Op::Reorder`] (`m`, in-place
//! permutation; may target a root array). [`op_from_json`] is the port of
//! `assertValidOp` and [`wire_op_from_json`] the port of
//! `assertValidWireOp`; each vocabulary keeps its own validator, exactly
//! like upstream.
//!
//! # Strings count UTF-16 code units
//!
//! Upstream strings are JS strings and `t`/`overlap` arithmetic counts
//! UTF-16 code units (README "Strings"). [`utf16_len`],
//! [`slice_utf16_from`] and [`overlap`] all work in code units, so ops
//! produced by the TypeScript implementation stay interoperable.
//!
//! # Canonical serialization note (disclosed divergence D1, continuing)
//!
//! Upstream JS objects iterate in insertion order; the port runs
//! `serde_json` with `preserve_order`, so wire bytes keep upstream's
//! insertion order. Oracle comparison still canonicalizes both sides with
//! recursively sorted object keys (see `tests/fixtures/chord_delta_oracle/`),
//! and oracle scenarios build multi-key objects in sorted key order.

mod apply_immutable_trusted;
mod codec;
mod diff;
mod revision_validator;
mod tracker;

pub use apply_immutable_trusted::apply_immutable_trusted;
pub use codec::{
    decoder, encoder, wire_op_from_json, wire_op_to_json, Decoder, Encoder, PathRef, WireOp,
};
pub use diff::diff_revisions;
pub use revision_validator::JsonRevisionValidator;
pub use tracker::{track, Change, Prepared, Tracker, TrackerError};

use std::fmt;

use serde_json::Number;

/// Upstream `JsonValue` (`packages/chord/src/types.ts:21`): the strict JSON
/// value union, mapped to `serde_json::Value`.
pub type JsonValue = serde_json::Value;

/// Upstream `Draft<T>` (`delta/draft.ts`): a mutable transaction-scoped view
/// of a JSON value. The TS mapped type describes the proxy draft handed to
/// `change` callbacks; the port's draft is the [`Change`] handle.
pub type Draft = Change;

/// One path segment: an object key or an array index. Port of `Seg =
/// string | number` (`delta/index.ts:14`); the number case is a non-negative
/// integer after validation (`assertSafePath`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Seg {
    /// An object key (upstream string segment).
    Key(String),
    /// An array index (upstream number segment).
    Index(usize),
}

impl From<&str> for Seg {
    fn from(key: &str) -> Self {
        Seg::Key(key.to_owned())
    }
}

impl From<String> for Seg {
    fn from(key: String) -> Self {
        Seg::Key(key)
    }
}

impl From<usize> for Seg {
    fn from(index: usize) -> Self {
        Seg::Index(index)
    }
}

impl fmt::Display for Seg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Seg::Key(key) => write!(f, "{key}"),
            Seg::Index(index) => write!(f, "{index}"),
        }
    }
}

/// A path into a nested JSON value. Port of `Path = readonly Seg[]`
/// (`delta/index.ts:15`).
pub type Path = Vec<Seg>;

/// One operation of the delta vocabulary. Port of the upstream `Op` tuple
/// union (`delta/index.ts:32-40`).
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// `["r", value]` — replace the complete value. The only op that may
    /// target the root.
    Replace(JsonValue),
    /// `["s", path, value]` — set a property or array element; non-root path.
    Set { path: Path, value: JsonValue },
    /// `["d", path]` — delete an object property (or remove an array element
    /// at the consumer); non-root path.
    Delete { path: Path },
    /// `["a", path, text]` — append to an existing string.
    Append { path: Path, text: String },
    /// `["t", path, count]` — remove `count` UTF-16 code units from the
    /// front of an existing string.
    Truncate { path: Path, count: usize },
    /// `["p", path, index, remove, items]` — splice an array; the path may
    /// be empty (a root array).
    Splice {
        path: Path,
        index: usize,
        remove: usize,
        items: Vec<JsonValue>,
    },
    /// `["m", path, permutation]` — reorder an array in place:
    /// `new[i] = old[permutation[i]]`; the path may be empty (a root array).
    Reorder { path: Path, permutation: Vec<usize> },
}

impl Op {
    /// The op's target path; `None` for [`Op::Replace`].
    pub fn path(&self) -> Option<&Path> {
        match self {
            Op::Replace(_) => None,
            Op::Set { path, .. }
            | Op::Delete { path }
            | Op::Append { path, .. }
            | Op::Truncate { path, .. }
            | Op::Splice { path, .. }
            | Op::Reorder { path, .. } => Some(path),
        }
    }

    /// The op verb, as in the upstream tuple's first element.
    pub fn verb(&self) -> &'static str {
        match self {
            Op::Replace(_) => "r",
            Op::Set { .. } => "s",
            Op::Delete { .. } => "d",
            Op::Append { .. } => "a",
            Op::Truncate { .. } => "t",
            Op::Splice { .. } => "p",
            Op::Reorder { .. } => "m",
        }
    }
}

/// Error taxonomy for the delta surface. Upstream throws `TypeError` for
/// shape violations, `UnsafePathError` for segments that would reach the
/// prototype chain, and `PathError` for paths that do not resolve.
/// [`Display`](fmt::Display) reproduces the upstream `error.message` text
/// byte-for-byte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeltaError {
    /// Upstream `TypeError` from op validation or a tracker guard.
    InvalidOp(String),
    /// Upstream `UnsafePathError`.
    UnsafePath { segment: String },
    /// Upstream `PathError`.
    UnresolvablePath { path: String },
}

impl DeltaError {
    /// The upstream `error.message` text.
    pub fn message(&self) -> String {
        match self {
            DeltaError::InvalidOp(message) => message.clone(),
            DeltaError::UnsafePath { segment } => format!("unsafe path segment: {segment}"),
            DeltaError::UnresolvablePath { path } => format!("unresolvable path: {path}"),
        }
    }
}

impl fmt::Display for DeltaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for DeltaError {}

/// Segments that would reach the prototype chain in a JS host. Port of
/// `RESERVED_SEGMENTS` (`delta/index.ts:131`): `JSON.parse` makes
/// `__proto__` an own property, but `parent[key] = value` in an applier
/// would pollute `Object.prototype`, so path segments are untrusted input.
pub const RESERVED_SEGMENTS: [&str; 3] = ["__proto__", "constructor", "prototype"];

pub(crate) fn is_reserved(segment: &str) -> bool {
    RESERVED_SEGMENTS.contains(&segment)
}

/// Port of `assertSafePath` (`delta/index.ts:279-287`): string segments
/// must not be reserved; number segments must be non-negative integers
/// (enforced by the [`Seg::Index`] type).
pub fn assert_safe_path(path: &[Seg]) -> Result<(), DeltaError> {
    for seg in path {
        if let Seg::Key(key) = seg {
            if is_reserved(key) {
                return Err(DeltaError::UnsafePath {
                    segment: key.clone(),
                });
            }
        }
    }
    Ok(())
}

/// `assertPermutation` (`delta/index.ts:199-208`): a distinct in-range
/// index list — a bijection.
pub(crate) fn assert_permutation(permutation: &[usize]) -> Result<(), DeltaError> {
    let mut seen = vec![false; permutation.len()];
    for &index in permutation {
        if index >= permutation.len() || seen[index] {
            return Err(DeltaError::InvalidOp(
                "m permutation is not a bijection".to_owned(),
            ));
        }
        seen[index] = true;
    }
    Ok(())
}

/// Validate one op's shape the way `assertValidOp` + `assertSafePath` do
/// inside `apply`: `p`/`m` may target the root, every other verb has a
/// non-empty safe path, and `m` permutations must be bijections.
pub fn validate_op(op: &Op) -> Result<(), DeltaError> {
    match op {
        Op::Replace(_) => Ok(()),
        Op::Splice { path, .. } => assert_safe_path(path),
        Op::Reorder { path, permutation } => {
            assert_safe_path(path)?;
            assert_permutation(permutation)
        }
        Op::Set { path, .. }
        | Op::Delete { path }
        | Op::Append { path, .. }
        | Op::Truncate { path, .. } => {
            if path.is_empty() {
                return Err(DeltaError::InvalidOp("path is empty".to_owned()));
            }
            assert_safe_path(path)
        }
    }
}

fn json_nonneg_int(value: &JsonValue, what: &str) -> Result<usize, DeltaError> {
    let number = value
        .as_f64()
        .ok_or_else(|| DeltaError::InvalidOp(what.to_owned()))?;
    if !number.is_finite() || number < 0.0 || number.fract() != 0.0 || number > usize::MAX as f64 {
        return Err(DeltaError::InvalidOp(what.to_owned()));
    }
    Ok(number as usize)
}

/// Parse one decoded op tuple, rejecting wire forms. Port of
/// `assertValidOp` plus the tuple-to-enum mapping. Wire-only shapes
/// (`["s", 1]`, `["d"]`, `["a", "x"]`, `["t", 2]`, `["p", 0, 0, []]`,
/// `["#", id, path]`) fail here, exactly as upstream: validating `Op`
/// against the wire grammar would be laxer than the type.
pub fn op_from_json(value: &JsonValue) -> Result<Op, DeltaError> {
    let tuple = value
        .as_array()
        .ok_or_else(|| DeltaError::InvalidOp("op is not a tuple".to_owned()))?;
    if tuple.is_empty() {
        return Err(DeltaError::InvalidOp("op is not a tuple".to_owned()));
    }
    let verb = tuple[0]
        .as_str()
        .ok_or_else(|| DeltaError::InvalidOp(format!("unknown op verb: {}", tuple[0])))?;
    let op = match verb {
        "r" => {
            if tuple.len() != 2 {
                return Err(DeltaError::InvalidOp("r arity".to_owned()));
            }
            Op::Replace(tuple[1].clone())
        }
        "s" => {
            if tuple.len() != 3 {
                return Err(DeltaError::InvalidOp("s arity".to_owned()));
            }
            Op::Set {
                path: path_from_json(&tuple[1], true)?,
                value: tuple[2].clone(),
            }
        }
        "d" => {
            if tuple.len() != 2 {
                return Err(DeltaError::InvalidOp("d arity".to_owned()));
            }
            Op::Delete {
                path: path_from_json(&tuple[1], true)?,
            }
        }
        "a" => {
            if tuple.len() != 3 || !tuple[2].is_string() {
                return Err(DeltaError::InvalidOp("a shape".to_owned()));
            }
            Op::Append {
                path: path_from_json(&tuple[1], true)?,
                text: tuple[2].as_str().expect("checked above").to_owned(),
            }
        }
        "t" => {
            if tuple.len() != 3 {
                return Err(DeltaError::InvalidOp("t shape".to_owned()));
            }
            let count = tuple[2]
                .as_f64()
                .filter(|n| n.is_finite() && n.fract() == 0.0 && *n >= 0.0)
                .ok_or_else(|| DeltaError::InvalidOp("t shape".to_owned()))?;
            Op::Truncate {
                path: path_from_json(&tuple[1], true)?,
                count: count as usize,
            }
        }
        "p" => {
            if tuple.len() != 5 {
                return Err(DeltaError::InvalidOp("p arity".to_owned()));
            }
            let items = tuple[4]
                .as_array()
                .ok_or_else(|| DeltaError::InvalidOp("p items".to_owned()))?;
            Op::Splice {
                path: path_from_json(&tuple[1], false)?,
                index: json_nonneg_int(&tuple[2], "p index")?,
                remove: json_nonneg_int(&tuple[3], "p remove")?,
                items: items.clone(),
            }
        }
        "m" => {
            if tuple.len() != 3 {
                return Err(DeltaError::InvalidOp("m arity".to_owned()));
            }
            let permutation = tuple[2]
                .as_array()
                .ok_or_else(|| DeltaError::InvalidOp("m permutation is not an array".to_owned()))?;
            let mut indices = Vec::with_capacity(permutation.len());
            for item in permutation {
                indices.push(json_nonneg_int(item, "m permutation is not a bijection")?);
            }
            Op::Reorder {
                path: path_from_json(&tuple[1], false)?,
                permutation: indices,
            }
        }
        // Silently skipping an unknown verb is how a newer producer's op
        // vanishes (`delta/index.ts:187-189`).
        other => return Err(DeltaError::InvalidOp(format!("unknown op verb: {other}"))),
    };
    validate_op(&op)?;
    Ok(op)
}

/// `assertPathArg` (`delta/index.ts:193-197`): a real array, non-empty when
/// the verb forbids the root, with safe segments.
fn path_from_json(value: &JsonValue, non_empty: bool) -> Result<Path, DeltaError> {
    let segments = value
        .as_array()
        .ok_or_else(|| DeltaError::InvalidOp("path is not an array".to_owned()))?;
    if non_empty && segments.is_empty() {
        return Err(DeltaError::InvalidOp("path is empty".to_owned()));
    }
    let mut path = Path::with_capacity(segments.len());
    for segment in segments {
        if let Some(key) = segment.as_str() {
            path.push(Seg::Key(key.to_owned()));
        } else {
            path.push(Seg::Index(json_nonneg_int(
                segment,
                "path segment is not a safe index",
            )?));
        }
    }
    Ok(path)
}

/// Serialize an op back to its upstream tuple form (`delta/index.ts:30-40`);
/// the inverse of [`op_from_json`] for valid ops.
pub fn op_to_json(op: &Op) -> JsonValue {
    let verb = |name: &str, mut fields: Vec<JsonValue>| {
        let mut tuple = vec![JsonValue::String(name.to_owned())];
        tuple.append(&mut fields);
        JsonValue::Array(tuple)
    };
    match op {
        Op::Replace(value) => verb("r", vec![value.clone()]),
        Op::Set { path, value } => verb("s", vec![path_to_json(path), value.clone()]),
        Op::Delete { path } => verb("d", vec![path_to_json(path)]),
        Op::Append { path, text } => verb(
            "a",
            vec![path_to_json(path), JsonValue::String(text.clone())],
        ),
        Op::Truncate { path, count } => verb(
            "t",
            vec![path_to_json(path), JsonValue::Number(Number::from(*count))],
        ),
        Op::Splice {
            path,
            index,
            remove,
            items,
        } => verb(
            "p",
            vec![
                path_to_json(path),
                JsonValue::Number(Number::from(*index)),
                JsonValue::Number(Number::from(*remove)),
                JsonValue::Array(items.clone()),
            ],
        ),
        Op::Reorder { path, permutation } => verb(
            "m",
            vec![
                path_to_json(path),
                JsonValue::Array(
                    permutation
                        .iter()
                        .map(|at| JsonValue::Number(Number::from(*at)))
                        .collect(),
                ),
            ],
        ),
    }
}

pub(crate) fn seg_to_json(seg: &Seg) -> JsonValue {
    match seg {
        Seg::Key(key) => JsonValue::String(key.clone()),
        Seg::Index(index) => JsonValue::Number(Number::from(*index)),
    }
}

pub(crate) fn path_to_json(path: &[Seg]) -> JsonValue {
    JsonValue::Array(path.iter().map(seg_to_json).collect())
}

/// A plain non-negative integer as a JSON number (argument-slot junk refs
/// in the tracker, permutation serialization).
pub(crate) fn number_json(value: usize) -> JsonValue {
    JsonValue::Number(Number::from(value))
}

/// `isReplace` (`delta/index.ts:70`).
pub fn is_replace(op: &Op) -> bool {
    matches!(op, Op::Replace(_))
}

/// A batch begins with a replacement. Port of `isBase` (`delta/index.ts:76`):
/// exact rather than a heuristic, because flush guarantees `r` is at index 0
/// or absent.
pub fn is_base(ops: &[Op]) -> bool {
    matches!(ops.first(), Some(Op::Replace(_)))
}

/// UTF-16 code unit length of a Rust string (JS `str.length`).
pub(crate) fn utf16_len(value: &str) -> usize {
    value.chars().map(char::len_utf16).sum()
}

/// JS `str.slice(start)` in UTF-16 code units: the substring from the code
/// unit at `start`, or `""` when `start` is past the end. If `start` would
/// split a surrogate pair, the slice starts at the next char boundary.
pub(crate) fn slice_utf16_from(value: &str, start: usize) -> &str {
    let mut units = 0usize;
    for (offset, ch) in value.char_indices() {
        if start <= units {
            return &value[offset..];
        }
        units += ch.len_utf16();
    }
    ""
}

/// Longest UTF-16 suffix of `a` that is a prefix of `b`. Port of `overlap`
/// (`delta/index.ts:87-110`): probe with a long head first (few candidates),
/// fall back to one code unit, bound the candidates, give up with 0 — which
/// emits a set: larger, never wrong.
pub fn overlap(a: &str, b: &str, scan: usize) -> usize {
    let a_units: Vec<u16> = a.encode_utf16().collect();
    let b_units: Vec<u16> = b.encode_utf16().collect();
    if a_units.is_empty() || b_units.is_empty() || scan == 0 {
        return 0;
    }
    let tail_len = a_units.len().min(scan);
    let tail = &a_units[a_units.len() - tail_len..];
    for head_len in [b_units.len().min(64), 1] {
        let head = &b_units[..head_len];
        let mut tried = 0usize;
        let mut from = 0usize;
        while let Some(at) = find_unit_slice(&tail[from..], head) {
            let k = from + at;
            tried += 1;
            if tried > 8 {
                break;
            }
            let n = tail.len() - k;
            if n <= b_units.len() && tail[k..] == b_units[..n] {
                return n;
            }
            from = k + 1;
        }
        if head_len == 1 {
            break;
        }
    }
    0
}

fn find_unit_slice(haystack: &[u16], needle: &[u16]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// JS `Array.prototype.splice(start, deleteCount)` clamping for the `p`
/// op: a start past the end appends, and the removal clamps to the tail
/// (deterministic and identical on both sides).
pub(crate) fn splice_clamped(
    array: &mut Vec<JsonValue>,
    index: usize,
    remove: usize,
    items: &[JsonValue],
) {
    let start = index.min(array.len());
    let end = (start + remove).min(array.len());
    array.drain(start..end);
    array.splice(start..start, items.to_vec());
}

fn unresolvable(path: &[Seg]) -> DeltaError {
    DeltaError::UnresolvablePath {
        path: serde_json::to_string(&path_to_json(path))
            .expect("path JSON serialization cannot fail"),
    }
}

/// Resolve `path` to a mutable container (object or array). Port of
/// `resolve` with `resolveValue`'s own-property walk.
fn resolve_container<'a>(
    root: &'a mut JsonValue,
    path: &[Seg],
) -> Result<&'a mut JsonValue, DeltaError> {
    let mut node = root;
    for seg in path {
        match seg {
            Seg::Key(key) => {
                let object = node.as_object_mut().ok_or_else(|| unresolvable(path))?;
                node = object.get_mut(key).ok_or_else(|| unresolvable(path))?;
            }
            Seg::Index(index) => {
                let array = node.as_array_mut().ok_or_else(|| unresolvable(path))?;
                node = array.get_mut(*index).ok_or_else(|| unresolvable(path))?;
            }
        }
    }
    if !node.is_object() && !node.is_array() {
        return Err(unresolvable(path));
    }
    Ok(node)
}

/// Apply ops to a plain mutable value. Port of `apply`/`applyOps`
/// (`delta/index.ts:326-406`). Upstream adopts `r` payloads and mutates in
/// place; the port takes the previous value by reference and returns a
/// fully owned result (the ownership equivalent — inputs are never mutated
/// or retained). Takes decoded ops: path ids and omitted paths are a wire
/// concern — run [`decoder().decode`](Decoder::decode) first for boundary
/// input.
pub fn apply(target: Option<&JsonValue>, ops: &[Op]) -> Result<JsonValue, DeltaError> {
    let mut root = target.cloned().unwrap_or(JsonValue::Null);
    for op in ops {
        validate_op(op)?;
        match op {
            Op::Replace(value) => {
                // Adopted, not copied, upstream; the clone is the ownership
                // equivalent in Rust.
                root = value.clone();
            }
            Op::Splice {
                path,
                index,
                remove,
                items,
            } => {
                let container = if path.is_empty() {
                    &mut root
                } else {
                    resolve_container(&mut root, path)?
                };
                let array = container.as_array_mut().ok_or_else(|| unresolvable(path))?;
                splice_clamped(array, *index, *remove, items);
            }
            Op::Reorder { path, permutation } => {
                let container = if path.is_empty() {
                    &mut root
                } else {
                    resolve_container(&mut root, path)?
                };
                let array = container.as_array_mut().ok_or_else(|| unresolvable(path))?;
                if array.len() != permutation.len() {
                    return Err(unresolvable(path));
                }
                let previous = array.clone();
                for (index, from) in permutation.iter().enumerate() {
                    array[index] = previous[*from].clone();
                }
            }
            Op::Set { .. } | Op::Delete { .. } | Op::Append { .. } | Op::Truncate { .. } => {
                let path = op.path().expect("non-replace op has a path");
                let (parent_path, key) = path.split_at(path.len() - 1);
                let parent = resolve_container(&mut root, parent_path)?;
                apply_leaf(parent, &key[0], op)?;
            }
        }
    }
    Ok(root)
}

/// Apply one already-validated non-replace op against the resolved parent.
/// Port of the `s`/`d`/`a`/`t` arms of `applyOps` plus the array-parent
/// guards (`assertIndexInRange`, `delta/index.ts:304-306`).
fn apply_leaf(parent: &mut JsonValue, key: &Seg, op: &Op) -> Result<(), DeltaError> {
    if parent.is_array() {
        let index = match key {
            Seg::Index(index) => *index,
            // A string segment under an array parent is rejected at the
            // consumer.
            Seg::Key(key) => {
                return Err(DeltaError::UnsafePath {
                    segment: key.clone(),
                });
            }
        };
        let array = parent.as_array_mut().expect("checked above");
        match op {
            // An index may address an existing element or append exactly one
            // past the end (`delta/index.ts:289-306`).
            Op::Set { value, .. } => {
                if index > array.len() {
                    return Err(DeltaError::UnsafePath {
                        segment: index.to_string(),
                    });
                }
                if index == array.len() {
                    array.push(value.clone());
                } else {
                    array[index] = value.clone();
                }
                Ok(())
            }
            Op::Delete { .. } => {
                if index >= array.len() {
                    return Err(unresolvable(op.path().expect("non-replace op has a path")));
                }
                array.remove(index);
                Ok(())
            }
            Op::Append { .. } | Op::Truncate { .. } => {
                let path = op.path().expect("non-replace op has a path");
                let existing = array
                    .get(index)
                    .ok_or_else(|| unresolvable(path))?
                    .as_str()
                    .ok_or_else(|| unresolvable(path))?;
                let updated = string_op_result(op, existing);
                array[index] = JsonValue::String(updated);
                Ok(())
            }
            Op::Replace(_) | Op::Splice { .. } | Op::Reorder { .. } => {
                unreachable!(
                    "replace handled by the caller; splice/permutation resolve their own path"
                )
            }
        }
    } else {
        let object = parent.as_object_mut().expect("checked above");
        let Seg::Key(key) = key else {
            return Err(DeltaError::UnsafePath {
                segment: key.to_string(),
            });
        };
        match op {
            Op::Set { value, .. } => {
                // defineProperty rather than assignment upstream — a plain
                // map insert cannot run inherited setters either; reserved
                // names are legal as VALUE keys.
                object.insert(key.clone(), value.clone());
                Ok(())
            }
            // JS `delete obj.missing` is a no-op.
            Op::Delete { .. } => {
                object.shift_remove(key);
                Ok(())
            }
            Op::Append { .. } | Op::Truncate { .. } => {
                let path = op.path().expect("non-replace op has a path");
                let existing = object
                    .get(key)
                    .ok_or_else(|| unresolvable(path))?
                    .as_str()
                    .ok_or_else(|| unresolvable(path))?;
                let updated = string_op_result(op, existing);
                object.insert(key.clone(), JsonValue::String(updated));
                Ok(())
            }
            Op::Replace(_) | Op::Splice { .. } | Op::Reorder { .. } => {
                unreachable!(
                    "replace handled by the caller; splice/permutation resolve their own path"
                )
            }
        }
    }
}

/// The `a`/`t` result over an existing string: append, or slice off the
/// front by UTF-16 code units.
fn string_op_result(op: &Op, existing: &str) -> String {
    match op {
        Op::Append { text, .. } => format!("{existing}{text}"),
        Op::Truncate { count, .. } => slice_utf16_from(existing, *count).to_owned(),
        _ => unreachable!("string arms only"),
    }
}

/// Apply decoded operations without mutating the previous immutable value.
/// Port of `applyImmutable` (`delta/index.ts:409-411`). Upstream copies only
/// the containers along each op's path and shares unchanged subtrees; owned
/// Rust trees have no structural sharing, so the port produces the same
/// value by application over an owned clone. `None` is upstream `undefined`
/// (a replica that has not hydrated yet).
pub fn apply_immutable(target: Option<&JsonValue>, ops: &[Op]) -> Result<JsonValue, DeltaError> {
    apply(target, ops)
}

/// Apply decoded operation batches as one final-result-only replay. Port of
/// `applyImmutableBatches` (`delta/index.ts:419-434`): containers copied for
/// an earlier batch may be mutated while applying a later batch, so no
/// intermediate revisions are exposed.
pub fn apply_immutable_batches<I>(
    target: Option<&JsonValue>,
    batches: I,
) -> Result<JsonValue, DeltaError>
where
    I: IntoIterator<Item = Vec<Op>>,
{
    let mut current = target.cloned().unwrap_or(JsonValue::Null);
    for ops in batches {
        current = apply(Some(&current), &ops)?;
    }
    Ok(current)
}

#[cfg(test)]
mod oracle_tests;
