//! Chord delta: operation-log change tracking over plain JSON. Port of
//! `packages/chord/src/delta/index.ts` (used surface: `Op`, `Path`, `Seg`,
//! `Tracker`/`track`, `applyImmutable`, `isBase`).
//!
//! # Operation vocabulary
//!
//! Upstream ops are JSON tuples (`delta/index.ts:30-36`, README "Operation
//! vocabulary"); the port models them as the [`Op`] enum with the same verb
//! set — `r` (replace the complete value), `s` (set a property/element), `d`
//! (delete an object property), `a` (append to a string), `t` (remove UTF-16
//! code units from a string's front), `p` (splice an array). Like upstream,
//! `s`/`d`/`a`/`t` cannot target the root ([`Op::Replace`] is the only
//! whole-value op) and `p` may address a root array. [`delta::op_from_json`]
//! is the port of `assertValidOp` (`delta/index.ts:1215-1249`) and is the only
//! way untrusted tuples (persisted JSONL, decoded envelopes) become ops.
//!
//! # String ops count UTF-16 code units
//!
//! Upstream strings are JS strings and `t` counts UTF-16 code units (README
//! "Strings"). The port keeps unit semantics — `utf16_len`,
//! `slice_utf16_from` and `overlap` all work in code units — so ops produced
//! or consumed by the TypeScript implementation remain interoperable across a
//! shared session file.
//!
//! # Tracker design (disclosed deviation)
//!
//! Upstream `track` records ops at write time through a `Proxy`, with
//! coalescing (a slot trie, tombstones, folding into ancestor payloads). Rust
//! has no proxies, so [`Tracker`] records nothing at write time:
//! [`Tracker::state_mut`] hands out the current value, and [`Tracker::flush`]
//! diffs the value at the previous flush against the current one using ports
//! of upstream's own diff functions (`diffValue`/`diffString`/`diffArray`/
//! `diffObject`, `delta/index.ts:200-308`) — the same functions upstream uses
//! for whole-container assignment and anchored strings at flush. The
//! contract that survives verbatim (`delta/README.md`): the first `flush` is
//! always a base batch; each later flush returns ops whose application
//! transforms the previously published value into the current one; `flush`
//! returns `[]` when nothing is pending; `rebase` forces the next flush to a
//! base batch; `discard` accepts pending mutations without publishing; the
//! operation sequence is not canonical and consumers must depend on the
//! resulting value, not the exact tuples.
//!
//! Upstream features that are unrepresentable or moot over owned
//! `serde_json::Value` trees, deferred to the full M6 port if ever needed:
//! write-time coalescing (subsumed by diffing the final value), structural
//! aliasing of one object at several positions, held-reference renumbering
//! across splices, and structural sharing in `applyImmutable` (the port clones
//! the previous value per call; inputs are never mutated).

use std::fmt;

use serde_json::{Map, Number};

use super::JsonValue;

/// One path segment: an object key or an array index. Port of upstream
/// `Seg = string | number` (`delta/index.ts:12`); the number case is always a
/// non-negative integer after validation (`assertSafePath`,
/// `delta/index.ts:1321-1329`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Seg {
    /// An object key (upstream string segment).
    Key(String),
    /// An array index (upstream number segment; JSON has no negative array
    /// indices).
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

/// A path into a nested JSON value. Port of upstream `Path = readonly Seg[]`
/// (`delta/index.ts:13`).
pub type Path = Vec<Seg>;

/// One operation of the delta vocabulary. Port of the upstream `Op` tuple
/// union (`delta/index.ts:30-36`); field names carry the tuple positions.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// `["r", value]` — replace the complete value. The only op that may
    /// target the root.
    Replace(JsonValue),
    /// `["s", path, value]` — set a property or array element. The path is
    /// non-empty.
    Set { path: Path, value: JsonValue },
    /// `["d", path]` — delete an object property (or remove an array element,
    /// shifting the tail). The path is non-empty.
    Delete { path: Path },
    /// `["a", path, text]` — append to an existing string.
    Append { path: Path, text: String },
    /// `["t", path, count]` — remove `count` UTF-16 code units from the front
    /// of an existing string.
    Truncate { path: Path, count: usize },
    /// `["p", path, index, remove, items]` — splice an array; the path may be
    /// empty (a root array).
    Splice {
        path: Path,
        index: usize,
        remove: usize,
        items: Vec<JsonValue>,
    },
}

impl Op {
    /// The op's target path; `None` for [`Op::Replace`] (it has no path).
    pub fn path(&self) -> Option<&Path> {
        match self {
            Op::Replace(_) => None,
            Op::Set { path, .. }
            | Op::Delete { path }
            | Op::Append { path, .. }
            | Op::Truncate { path, .. }
            | Op::Splice { path, .. } => Some(path),
        }
    }
}

/// Error taxonomy for the delta surface. Upstream uses three classes:
/// `TypeError` for shape violations (`assertValidOp`, `delta/index.ts:1215-1319`),
/// `UnsafePathError` (`delta/index.ts:1196-1205`) for segments that would
/// reach the prototype chain or address arrays wrongly, and `PathError`
/// (`delta/index.ts:1352-1359`) for paths that do not resolve. `Apply` errors
/// terminate the stream (README "State ownership") — callers discard the
/// replica and recover from a later base batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeltaError {
    /// Upstream `TypeError` from op validation.
    InvalidOp(String),
    /// Upstream `UnsafePathError`: reserved segment or a wrong segment type
    /// for the value it addresses.
    UnsafePath { segment: String },
    /// Upstream `PathError`: the path does not resolve against the value.
    UnresolvablePath { path: String },
}

impl fmt::Display for DeltaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeltaError::InvalidOp(message) => write!(f, "invalid op: {message}"),
            DeltaError::UnsafePath { segment } => write!(f, "unsafe path segment: {segment}"),
            DeltaError::UnresolvablePath { path } => write!(f, "unresolvable path: {path}"),
        }
    }
}

impl std::error::Error for DeltaError {}

/// Segments that would reach the prototype chain in a JS host. Port of
/// `RESERVED_SEGMENTS` (`delta/index.ts:1194`). Ops arrive from persisted
/// files or tool output, so none of it is trusted input; the guard applies to
/// path segments only — reserved names remain legal as value keys
/// (`delta/index.ts` "allows a reserved name as a VALUE key" test).
pub const RESERVED_SEGMENTS: [&str; 3] = ["__proto__", "constructor", "prototype"];

fn is_reserved(segment: &str) -> bool {
    RESERVED_SEGMENTS.contains(&segment)
}

/// Validate an op's shape and path the way `assertValidOp` +
/// `assertSafePath` (`delta/index.ts:1215-1249,1321-1329`) do inside `apply`.
/// Ops built programmatically are validated here at apply time, exactly like
/// upstream validates every op regardless of its static type.
pub fn validate_op(op: &Op) -> Result<(), DeltaError> {
    match op {
        Op::Replace(_) => Ok(()),
        Op::Splice { path, .. } => {
            // p may target the root: the path may be empty
            // (delta/index.ts:30-36, 1237-1244).
            assert_safe_path(path)
        }
        Op::Set { path, .. }
        | Op::Delete { path }
        | Op::Append { path, .. }
        | Op::Truncate { path, .. } => {
            // s/d/a/t can never target the root — the type forbids it
            // (delta/index.ts:1403).
            if path.is_empty() {
                return Err(DeltaError::InvalidOp("path is empty".to_owned()));
            }
            assert_safe_path(path)
        }
    }
}

/// Port of `assertSafePath` (`delta/index.ts:1321-1329`): string segments must
/// not be reserved; number segments must be non-negative integers (the
/// [`Seg::Index`] type enforces integrality, so only the reserved check does
/// real work here).
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

/// A non-negative integer in a JSON number field (`t` count, `p` index/remove,
/// path index segments). JS accepts `3.0` as the integer 3; so does this.
fn json_nonneg_int(value: &JsonValue, what: &str) -> Result<usize, DeltaError> {
    let number = value
        .as_f64()
        .ok_or_else(|| DeltaError::InvalidOp(format!("{what} is not a number")))?;
    if !number.is_finite() || number < 0.0 || number.fract() != 0.0 || number > usize::MAX as f64 {
        return Err(DeltaError::InvalidOp(format!(
            "{what} must be a non-negative integer"
        )));
    }
    Ok(number as usize)
}

/// Parse one decoded op tuple, rejecting wire forms. Port of `assertValidOp`
/// (`delta/index.ts:1215-1249`) plus the tuple-to-enum mapping; this is the
/// boundary for persisted or received op tuples. Wire-only shapes (`["s", 1]`,
/// `["d"]`, `["a", "x"]`, `["t", 2]`, `["p", 0, 0, []]`, `["#", id, path]`)
/// fail here, exactly as upstream: validating `Op` against the wire grammar
/// would be laxer than the type (`delta/index.ts:1207-1214`).
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
            Op::Truncate {
                path: path_from_json(&tuple[1], true)?,
                count: json_nonneg_int(&tuple[2], "t count must be a non-negative integer")?,
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
        // Silently skipping an unknown verb is how a newer producer's op
        // vanishes (delta/index.ts:1245-1247).
        other => {
            return Err(DeltaError::InvalidOp(format!("unknown op verb: {other}")));
        }
    };
    validate_op(&op)?;
    Ok(op)
}

/// `assertPathArg` (`delta/index.ts:1251-1255`): a real array, non-empty when
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

/// Serialize an op back to its upstream tuple form
/// (`delta/index.ts:30-36`). The inverse of [`op_from_json`] for valid ops.
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
    }
}

fn seg_to_json(seg: &Seg) -> JsonValue {
    match seg {
        Seg::Key(key) => JsonValue::String(key.clone()),
        Seg::Index(index) => JsonValue::Number(Number::from(*index)),
    }
}

fn path_to_json(path: &[Seg]) -> JsonValue {
    JsonValue::Array(path.iter().map(seg_to_json).collect())
}

/// A batch begins with a replacement. Port of `isBase`
/// (`delta/index.ts:66-70`): exact rather than a heuristic, because flush
/// guarantees `r` is at index 0 or absent.
pub fn is_base(ops: &[Op]) -> bool {
    matches!(ops.first(), Some(Op::Replace(_)))
}

/// Apply decoded operations without mutating the previous immutable value.
/// Port of `applyImmutable` (`delta/index.ts:1444-1456`).
///
/// `None` is upstream `undefined` (a replica that has not hydrated yet); the
/// first `r` op seeds it. On success the returned value is fully owned; the
/// inputs (previous value and ops) are never mutated. An error leaves `target`
/// untouched and terminates the stream — recover from a later base batch
/// (README "State ownership").
pub fn apply_immutable(target: Option<&JsonValue>, ops: &[Op]) -> Result<JsonValue, DeltaError> {
    // Upstream starts from the target and copies only containers along each
    // op's path; serde_json values are owned trees with no structural
    // sharing, so the port clones the previous value once per call and
    // applies each op in place. Inputs are never mutated either way.
    let mut root = target.cloned().unwrap_or(JsonValue::Null);
    for op in ops {
        validate_op(op)?;
        match op {
            Op::Replace(value) => {
                // Adopted, not copied, upstream (delta/index.ts:1377-1386);
                // the port's clone is the ownership equivalent — the caller's
                // batch is never aliased or mutated.
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

/// JS `Array.prototype.splice(start, deleteCount)` clamping for the `p` op:
/// a start past the end appends, and the removal clamps to the tail
/// (`delta/index.ts` "clamps a splice remove past the end" test).
fn splice_clamped(array: &mut Vec<JsonValue>, index: usize, remove: usize, items: &[JsonValue]) {
    let start = index.min(array.len());
    let end = (start + remove).min(array.len());
    array.drain(start..end);
    array.splice(start..start, items.to_vec());
}

/// Resolve `path` to a mutable container (object or array). Port of `resolve`
/// (`delta/index.ts:1509-1513`) with `resolveValue`'s own-property walk
/// (`delta/index.ts:1496-1507`).
fn resolve_container<'a>(
    root: &'a mut JsonValue,
    path: &[Seg],
) -> Result<&'a mut JsonValue, DeltaError> {
    let mut node = root;
    for seg in path {
        match seg {
            Seg::Key(key) => {
                let object = node.as_object_mut().ok_or_else(|| unresolvable(path))?;
                let child = object.get_mut(key).ok_or_else(|| unresolvable(path))?;
                node = child;
            }
            Seg::Index(index) => {
                let array = node.as_array_mut().ok_or_else(|| unresolvable(path))?;
                let child = array.get_mut(*index).ok_or_else(|| unresolvable(path))?;
                node = child;
            }
        }
    }
    if !node.is_object() && !node.is_array() {
        return Err(unresolvable(path));
    }
    Ok(node)
}

fn unresolvable(path: &[Seg]) -> DeltaError {
    DeltaError::UnresolvablePath {
        path: format!("{path:?}"),
    }
}

/// Apply one already-validated non-replace op in place against the resolved
/// parent. Port of the `s`/`d`/`a`/`t` arms of `applyOps`
/// (`delta/index.ts:1404-1438`) plus the array-parent guards
/// (`delta/index.ts:1406-1409`, `assertIndexInRange` at 1346-1348).
fn apply_leaf(parent: &mut JsonValue, key: &Seg, op: &Op) -> Result<(), DeltaError> {
    if parent.is_array() {
        let index = match key {
            Seg::Index(index) => *index,
            // A string segment under an array parent is rejected at the
            // consumer (delta/index.ts "rejects string-spelled array indices
            // at the consumer").
            Seg::Key(key) => {
                return Err(DeltaError::UnsafePath {
                    segment: key.clone(),
                });
            }
        };
        let array = parent.as_array_mut().expect("checked above");
        match op {
            // An index may address an existing element or append exactly one
            // past the end (delta/index.ts:1331-1345).
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
            Op::Replace(_) | Op::Splice { .. } => {
                unreachable!("replace handled by the caller; splice resolves its own path")
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
                object.insert(key.clone(), value.clone());
                Ok(())
            }
            // JS `delete obj.missing` is a no-op (delta/index.ts:1424).
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
            Op::Replace(_) | Op::Splice { .. } => {
                unreachable!("replace handled by the caller; splice resolves its own path")
            }
        }
    }
}

/// The `a`/`t` result over an existing string: append, or slice off the front
/// by UTF-16 code units (`delta/index.ts:1426-1436`).
fn string_op_result(op: &Op, existing: &str) -> String {
    match op {
        Op::Append { text, .. } => format!("{existing}{text}"),
        Op::Truncate { count, .. } => slice_utf16_from(existing, *count).to_owned(),
        _ => unreachable!("string arms only"),
    }
}

/// UTF-16 code unit length of a Rust string (JS `str.length`). Crate-visible:
/// not an upstream export, but string ops count UTF-16 units everywhere.
pub(crate) fn utf16_len(value: &str) -> usize {
    value.chars().map(char::len_utf16).sum()
}

/// JS `str.slice(start)` in UTF-16 code units: the substring from the code
/// unit at `start`, or `""` when `start` is past the end. If `start` would
/// split a surrogate pair (only possible for cross-implementation ops), the
/// slice starts at the next char boundary.
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
/// (`delta/index.ts:74-104`): probe with a long head first (few candidates),
/// fall back to one code unit, bound the candidates, and give up with 0 —
/// which emits a set: larger, never wrong.
fn overlap(a: &str, b: &str, scan: usize) -> usize {
    // Work in UTF-16 code units so the returned count matches upstream's
    // JS-string arithmetic exactly, including astral content.
    let a_units: Vec<u16> = a.encode_utf16().collect();
    let b_units: Vec<u16> = b.encode_utf16().collect();
    if a_units.is_empty() || b_units.is_empty() || scan == 0 {
        return 0;
    }
    let tail_len = a_units.len().min(scan);
    let tail = &a_units[a_units.len() - tail_len..];
    // A probe of length h can only find overlaps of at least h — try a long
    // head first (few candidates), then one unit, which finds any overlap at
    // the cost of more candidates (delta/index.ts:87-93).
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
    // Giving up returns 0, which emits a set: larger, never wrong
    // (delta/index.ts:91-93).
    0
}

fn find_unit_slice(haystack: &[u16], needle: &[u16]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Tracker options. Port of `TrackerOptions`
/// (`delta/index.ts:108-110`); `max_overlap_scan` defaults to 65,536
/// (`delta/index.ts:311`).
#[derive(Clone, Copy, Debug)]
pub struct TrackerOptions {
    pub max_overlap_scan: usize,
}

impl Default for TrackerOptions {
    fn default() -> Self {
        TrackerOptions {
            max_overlap_scan: 65_536,
        }
    }
}

/// Tracker over a JSON value. Port of the upstream `Tracker<T>` interface
/// (`delta/index.ts:112-127`) and the flush/rebase/discard lifecycle
/// (`delta/index.ts:1128-1178`); see the module docs for the write-recording
/// deviation (the port diffs at flush with upstream's own diff functions).
#[derive(Debug, Clone)]
pub struct Tracker {
    baseline: JsonValue,
    state: JsonValue,
    force_base: bool,
    has_pending: bool,
    scan: usize,
}

/// `track(root)`: take ownership of `root` and start tracking. Port of
/// `track<T extends object>` (`delta/index.ts:310`); upstream restricts the
/// root to objects/arrays, the port accepts any JSON value (scalars work —
/// the first flush is a base batch either way).
pub fn track(root: JsonValue) -> Tracker {
    Tracker::with_options(root, TrackerOptions::default())
}

impl Tracker {
    /// `track(root, options)` (`delta/index.ts:310-311`).
    pub fn with_options(root: JsonValue, options: TrackerOptions) -> Tracker {
        Tracker {
            baseline: root.clone(),
            state: root,
            // forceBase = true: the first flush is a complete base batch
            // (delta/index.ts:336).
            force_base: true,
            has_pending: false,
            scan: options.max_overlap_scan,
        }
    }

    /// Upstream `tracker.state` reads / `tracker.target`: the current value.
    pub fn state(&self) -> &JsonValue {
        &self.state
    }

    /// Upstream mutating through `tracker.state`: hand out the current value
    /// and mark the tracker dirty. Conservative, like upstream's own `dirty`
    /// (`delta/index.ts:1150-1154`): true even if the writes cancel out.
    pub fn state_mut(&mut self) -> &mut JsonValue {
        self.has_pending = true;
        &mut self.state
    }

    /// Upstream `tracker.state = next` (`delta/index.ts:1135-1145`): replace
    /// the whole value; the next flush is a base batch and pending deltas are
    /// dropped.
    pub fn set_value(&mut self, next: JsonValue) {
        self.has_pending = false;
        self.state = next;
        self.force_base = true;
    }

    /// Upstream `rebase()` (`delta/index.ts:1146-1149`): make the next flush a
    /// complete base batch without changing the value.
    pub fn rebase(&mut self) {
        self.has_pending = false;
        self.force_base = true;
    }

    /// Upstream `discard()` (`delta/index.ts:1155-1157`): accept pending
    /// mutations locally without emitting them. In the port the pending
    /// window is `baseline != state`, so accepting means advancing the
    /// baseline; upstream's `forceBase` bookkeeping is untouched.
    pub fn discard(&mut self) {
        self.has_pending = false;
        self.baseline = self.state.clone();
    }

    /// Upstream `dirty` (`delta/index.ts:1150-1154`): true if anything was
    /// written since the last flush, or a base batch is still owed.
    pub fn is_dirty(&self) -> bool {
        self.force_base || self.has_pending
    }

    /// Upstream `flush()` (`delta/index.ts:1158-1177`): the first flush is a
    /// base batch `[r state]`; later flushes return ops whose application
    /// transforms the previously flushed value into the current one, and `[]`
    /// when nothing is pending.
    pub fn flush(&mut self) -> Vec<Op> {
        if self.force_base {
            self.force_base = false;
            self.has_pending = false;
            let value = self.state.clone();
            self.baseline = value.clone();
            return vec![Op::Replace(value)];
        }
        if !self.has_pending {
            return Vec::new();
        }
        let mut out = Vec::new();
        diff_value(
            Some(&self.baseline),
            Some(&self.state),
            &[],
            self.scan,
            &mut out,
        );
        self.baseline = self.state.clone();
        self.has_pending = false;
        out
    }
}

// ─── Diff (flush-side) ───────────────────────────────────────────────────────
//
// Direct ports of upstream's own diff functions
// (`delta/index.ts:200-308`), used at flush to turn baseline -> state into
// intent ops: appends for grown strings, truncate/append pairs for rolling
// windows, splices for arrays, per-key sets/deletes for objects.

fn emit_set(path: &[Seg], value: &JsonValue, out: &mut Vec<Op>) {
    if path.is_empty() {
        out.push(Op::Replace(value.clone()));
    } else {
        out.push(Op::Set {
            path: path.to_owned(),
            value: value.clone(),
        });
    }
}

fn emit_delete(path: &[Seg], out: &mut Vec<Op>) {
    // The tracked root cannot be deleted (delta/index.ts:195-198); the diff
    // only ever deletes object keys, which have non-empty paths.
    debug_assert!(!path.is_empty());
    if path.is_empty() {
        return;
    }
    out.push(Op::Delete {
        path: path.to_owned(),
    });
}

/// Port of `diffValue` (`delta/index.ts:224-247`). `None` is upstream's
/// MISSING sentinel (a key absent from the before-object).
fn diff_value(
    before: Option<&JsonValue>,
    after: Option<&JsonValue>,
    path: &[Seg],
    scan: usize,
    out: &mut Vec<Op>,
) {
    let (Some(before), Some(after)) = (before, after) else {
        // Upstream MISSING handling (delta/index.ts:225-232): a key absent
        // before is a set, a key absent after is a delete.
        if let Some(after) = after {
            emit_set(path, after, out);
        } else if before.is_some() {
            emit_delete(path, out);
        }
        return;
    };
    if before == after {
        return;
    }
    match (before, after) {
        (JsonValue::String(before), JsonValue::String(after)) => {
            diff_string(before, after, path, scan, out);
        }
        (JsonValue::Array(before), JsonValue::Array(after)) => {
            diff_array(before, after, path, scan, out);
        }
        (JsonValue::Object(before), JsonValue::Object(after)) => {
            diff_object(before, after, path, scan, out);
        }
        _ => emit_set(path, after, out),
    }
}

/// Port of `diffString` (`delta/index.ts:200-222`).
fn diff_string(before: &str, after: &str, path: &[Seg], scan: usize, out: &mut Vec<Op>) {
    if before == after {
        return;
    }
    if path.is_empty() {
        emit_set(path, &JsonValue::String(after.to_owned()), out);
        return;
    }
    let before_units = utf16_len(before);
    // A byte-prefix of a UTF-8 string is exactly its code-unit prefix, so
    // `starts_with` is `after.slice(0, before.length) === before`
    // (delta/index.ts:211-214).
    if after.len() > before.len() && after.starts_with(before) {
        out.push(Op::Append {
            path: path.to_owned(),
            text: after[before.len()..].to_owned(),
        });
        return;
    }
    let shared = overlap(before, after, scan);
    if shared == 0 {
        out.push(Op::Set {
            path: path.to_owned(),
            value: JsonValue::String(after.to_owned()),
        });
        return;
    }
    out.push(Op::Truncate {
        path: path.to_owned(),
        count: before_units - shared,
    });
    if utf16_len(after) > shared {
        out.push(Op::Append {
            path: path.to_owned(),
            text: slice_utf16_from(after, shared).to_owned(),
        });
    }
}

/// Port of `diffArray` (`delta/index.ts:268-308`).
fn diff_array(
    before: &[JsonValue],
    after: &[JsonValue],
    path: &[Seg],
    scan: usize,
    out: &mut Vec<Op>,
) {
    if before.len() == after.len() {
        for (index, item) in after.iter().enumerate() {
            let mut sub = path.to_owned();
            sub.push(Seg::Index(index));
            diff_value(Some(&before[index]), Some(item), &sub, scan, out);
        }
        return;
    }

    let mut prefix = 0;
    while prefix < before.len() && prefix < after.len() && before[prefix] == after[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < before.len() - prefix
        && suffix < after.len() - prefix
        && before[before.len() - 1 - suffix] == after[after.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let shorter = before.len().min(after.len());
    if prefix + suffix == shorter {
        let remove = before.len() - prefix - suffix;
        let items: Vec<JsonValue> = after[prefix..after.len() - suffix].to_vec();
        if prefix == 0 && remove == before.len() {
            emit_set(path, &JsonValue::Array(after.to_vec()), out);
        } else {
            out.push(Op::Splice {
                path: path.to_owned(),
                index: prefix,
                remove,
                items,
            });
        }
        return;
    }

    // Structural movement combined with retained-index edits has no unique
    // alignment. Preserve the retained index deltas and express only the tail
    // length change structurally (delta/index.ts:295-307).
    for index in 0..shorter {
        let mut sub = path.to_owned();
        sub.push(Seg::Index(index));
        diff_value(Some(&before[index]), Some(&after[index]), &sub, scan, out);
    }
    if after.len() > before.len() {
        out.push(Op::Splice {
            path: path.to_owned(),
            index: before.len(),
            remove: 0,
            items: after[before.len()..].to_vec(),
        });
    } else if before.len() > after.len() {
        if after.is_empty() {
            emit_set(path, &JsonValue::Array(Vec::new()), out);
        } else {
            out.push(Op::Splice {
                path: path.to_owned(),
                index: after.len(),
                remove: before.len() - after.len(),
                items: Vec::new(),
            });
        }
    }
}

/// Port of `diffObject` (`delta/index.ts:249-266`). Reserved keys anywhere in
/// either object degrade the whole subtree to a set
/// (`delta/index.ts:256-259`).
fn diff_object(
    before: &Map<String, JsonValue>,
    after: &Map<String, JsonValue>,
    path: &[Seg],
    scan: usize,
    out: &mut Vec<Op>,
) {
    if before
        .keys()
        .chain(after.keys())
        .any(|key| is_reserved(key))
    {
        emit_set(path, &JsonValue::Object(after.clone()), out);
        return;
    }
    for (key, value) in after {
        let mut sub = path.to_owned();
        sub.push(Seg::Key(key.clone()));
        diff_value(before.get(key), Some(value), &sub, scan, out);
    }
    for key in before.keys() {
        if !after.contains_key(key) {
            let mut sub = path.to_owned();
            sub.push(Seg::Key(key.clone()));
            emit_delete(&sub, out);
        }
    }
}

#[cfg(test)]
mod tests;
