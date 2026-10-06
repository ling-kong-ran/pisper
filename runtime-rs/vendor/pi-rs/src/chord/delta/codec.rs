//! Wire codec for delta ops: path interning and arity omission. Port of the
//! `Encoder`/`Decoder` section of `packages/chord/src/delta/index.ts`
//! (`delta/index.ts:1515-1697`).
//!
//! `WireOp` adds two compressions and nothing else: `["#", id, path]` defines
//! an id, emitted on a path's SECOND use; a numeric path reference addresses a
//! previously defined id; a shortened tuple reuses the previous op's path with
//! arity disambiguating. `["r", value]` carries no path, so it encodes to
//! itself, which is why `is_base` works unchanged on either vocabulary.
//!
//! ONE PAIR PER INDEPENDENT STATE STREAM (`delta/index.ts:1517-1523`): every
//! decoder must observe exactly the batches encoded by its matching encoder,
//! beginning with that state's base.

use std::collections::{HashMap, HashSet};

use serde_json::Number;

use super::{assert_safe_path, path_to_json, unresolvable, DeltaError, JsonValue, Op, Path, Seg};

/// A path inline, or an id assigned by the encoder on second use. Port of
/// `PathRef` (`delta/index.ts:17`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathRef {
    Path(Path),
    Id(u64),
}

/// What crosses a boundary. Port of `WireOp` (`delta/index.ts:48-60`); the
/// upstream arity-based tuple variants become enum variants, and
/// [`wire_op_to_json`] restores the exact tuples.
#[derive(Clone, Debug, PartialEq)]
pub enum WireOp {
    /// `["r", value]`
    Replace(JsonValue),
    /// `["s", ref, value]`
    Set { r: PathRef, value: JsonValue },
    /// `["s", value]` — short form, previous path reused
    SetShort(JsonValue),
    /// `["d", ref]`
    Delete { r: PathRef },
    /// `["d"]`
    DeleteShort,
    /// `["a", ref, text]`
    Append { r: PathRef, text: String },
    /// `["a", text]`
    AppendShort(String),
    /// `["t", ref, count]`
    Truncate { r: PathRef, count: usize },
    /// `["t", count]`
    TruncateShort(usize),
    /// `["p", ref, index, remove, items]`
    Splice {
        r: PathRef,
        index: usize,
        remove: usize,
        items: Vec<JsonValue>,
    },
    /// `["p", index, remove, items]`
    SpliceShort {
        index: usize,
        remove: usize,
        items: Vec<JsonValue>,
    },
    /// `["m", ref, permutation]`
    Reorder { r: PathRef, permutation: Vec<usize> },
    /// `["m", permutation]`
    ReorderShort(Vec<usize>),
    /// `["#", id, path]`
    Define { id: u64, path: Path },
}

fn wire_ref_json(r: &PathRef) -> JsonValue {
    match r {
        PathRef::Path(path) => path_to_json(path),
        PathRef::Id(id) => JsonValue::Number(Number::from(*id)),
    }
}

/// Restore the exact upstream wire tuple for a [`WireOp`].
pub fn wire_op_to_json(op: &WireOp) -> JsonValue {
    let verb = |name: &str, fields: Vec<JsonValue>| {
        let mut tuple = vec![JsonValue::String(name.to_owned())];
        tuple.extend(fields);
        JsonValue::Array(tuple)
    };
    match op {
        WireOp::Replace(value) => verb("r", vec![value.clone()]),
        WireOp::Set { r, value } => verb("s", vec![wire_ref_json(r), value.clone()]),
        WireOp::SetShort(value) => verb("s", vec![value.clone()]),
        WireOp::Delete { r } => verb("d", vec![wire_ref_json(r)]),
        WireOp::DeleteShort => verb("d", vec![]),
        WireOp::Append { r, text } => {
            verb("a", vec![wire_ref_json(r), JsonValue::String(text.clone())])
        }
        WireOp::AppendShort(text) => verb("a", vec![JsonValue::String(text.clone())]),
        WireOp::Truncate { r, count } => verb(
            "t",
            vec![wire_ref_json(r), JsonValue::Number(Number::from(*count))],
        ),
        WireOp::TruncateShort(count) => verb("t", vec![JsonValue::Number(Number::from(*count))]),
        WireOp::Splice {
            r,
            index,
            remove,
            items,
        } => verb(
            "p",
            vec![
                wire_ref_json(r),
                JsonValue::Number(Number::from(*index)),
                JsonValue::Number(Number::from(*remove)),
                JsonValue::Array(items.clone()),
            ],
        ),
        WireOp::SpliceShort {
            index,
            remove,
            items,
        } => verb(
            "p",
            vec![
                JsonValue::Number(Number::from(*index)),
                JsonValue::Number(Number::from(*remove)),
                JsonValue::Array(items.clone()),
            ],
        ),
        WireOp::Reorder { r, permutation } => {
            verb("m", vec![wire_ref_json(r), permutation_json(permutation)])
        }
        WireOp::ReorderShort(permutation) => verb("m", vec![permutation_json(permutation)]),
        WireOp::Define { id, path } => verb(
            "#",
            vec![JsonValue::Number(Number::from(*id)), path_to_json(path)],
        ),
    }
}

/// `m` permutations serialize as plain number arrays.
fn permutation_json(permutation: &[usize]) -> JsonValue {
    JsonValue::Array(
        permutation
            .iter()
            .map(|at| JsonValue::Number(Number::from(*at)))
            .collect(),
    )
}

fn wire_nonneg_int(value: &JsonValue, what: &str) -> Result<usize, DeltaError> {
    let number = value
        .as_f64()
        .ok_or_else(|| DeltaError::InvalidOp(what.to_owned()))?;
    if !number.is_finite() || number < 0.0 || number.fract() != 0.0 || number > usize::MAX as f64 {
        return Err(DeltaError::InvalidOp(what.to_owned()));
    }
    Ok(number as usize)
}

fn wire_ref_from_json(value: &JsonValue) -> Result<PathRef, DeltaError> {
    if value.is_number() {
        // Number.isInteger(r) && r >= 0 ("bad path id").
        return match value.as_u64() {
            Some(id) => Ok(PathRef::Id(id)),
            None => Err(DeltaError::InvalidOp("bad path id".to_owned())),
        };
    }
    let segments = value
        .as_array()
        .ok_or_else(|| DeltaError::InvalidOp("path is not an array".to_owned()))?;
    // A string is not a path. Unchecked, `"a".slice(0, -1)` is `""`, so it
    // resolves to the ROOT and writes there (delta/index.ts:1266-1270).
    let mut path = Path::with_capacity(segments.len());
    for segment in segments {
        if let Some(key) = segment.as_str() {
            path.push(Seg::Key(key.to_owned()));
        } else {
            path.push(Seg::Index(wire_nonneg_int(
                segment,
                "path segment is not a safe index",
            )?));
        }
    }
    assert_safe_path(&path)?;
    Ok(PathRef::Path(path))
}

/// Parse one wire op tuple with the wire grammar's checks. Port of
/// `assertValidWireOp` (`delta/index.ts:1258-1319`).
pub fn wire_op_from_json(value: &JsonValue) -> Result<WireOp, DeltaError> {
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
            WireOp::Replace(tuple[1].clone())
        }
        "s" => {
            if tuple.len() == 3 {
                WireOp::Set {
                    r: wire_ref_from_json(&tuple[1])?,
                    value: tuple[2].clone(),
                }
            } else if tuple.len() == 2 {
                WireOp::SetShort(tuple[1].clone())
            } else {
                return Err(DeltaError::InvalidOp("s arity".to_owned()));
            }
        }
        "d" => {
            if tuple.len() == 2 {
                WireOp::Delete {
                    r: wire_ref_from_json(&tuple[1])?,
                }
            } else if tuple.len() == 1 {
                WireOp::DeleteShort
            } else {
                return Err(DeltaError::InvalidOp("d arity".to_owned()));
            }
        }
        "a" => {
            if tuple.len() == 3 {
                WireOp::Append {
                    r: wire_ref_from_json(&tuple[1])?,
                    text: tuple[2]
                        .as_str()
                        .ok_or_else(|| DeltaError::InvalidOp("a value".to_owned()))?
                        .to_owned(),
                }
            } else if tuple.len() == 2 {
                WireOp::AppendShort(
                    tuple[1]
                        .as_str()
                        .ok_or_else(|| DeltaError::InvalidOp("a value".to_owned()))?
                        .to_owned(),
                )
            } else {
                return Err(DeltaError::InvalidOp("a arity".to_owned()));
            }
        }
        "t" => {
            if tuple.len() == 3 {
                let count = tuple[2]
                    .as_f64()
                    .filter(|n| n.is_finite() && n.fract() == 0.0 && *n >= 0.0)
                    .ok_or_else(|| DeltaError::InvalidOp("t count".to_owned()))?;
                WireOp::Truncate {
                    r: wire_ref_from_json(&tuple[1])?,
                    count: count as usize,
                }
            } else if tuple.len() == 2 {
                let count = tuple[1]
                    .as_f64()
                    .filter(|n| n.is_finite() && n.fract() == 0.0 && *n >= 0.0)
                    .ok_or_else(|| DeltaError::InvalidOp("t count".to_owned()))?;
                WireOp::TruncateShort(count as usize)
            } else {
                return Err(DeltaError::InvalidOp("t arity".to_owned()));
            }
        }
        "p" => {
            if tuple.len() != 4 && tuple.len() != 5 {
                return Err(DeltaError::InvalidOp("p arity".to_owned()));
            }
            let (r, i, rm, items) = if tuple.len() == 5 {
                (
                    Some(wire_ref_from_json(&tuple[1])?),
                    wire_nonneg_int(&tuple[2], "p index")?,
                    wire_nonneg_int(&tuple[3], "p remove")?,
                    &tuple[4],
                )
            } else {
                (
                    None,
                    wire_nonneg_int(&tuple[1], "p index")?,
                    wire_nonneg_int(&tuple[2], "p remove")?,
                    &tuple[3],
                )
            };
            let items = items
                .as_array()
                .ok_or_else(|| DeltaError::InvalidOp("p items".to_owned()))?
                .clone();
            match r {
                Some(r) => WireOp::Splice {
                    r,
                    index: i,
                    remove: rm,
                    items,
                },
                None => WireOp::SpliceShort {
                    index: i,
                    remove: rm,
                    items,
                },
            }
        }
        "m" => {
            // `["m", ref, permutation]` / `["m", permutation]`; the
            // permutation is always the final element
            // (delta/index.ts:1261-1264).
            if tuple.len() == 3 {
                WireOp::Reorder {
                    r: wire_ref_from_json(&tuple[1])?,
                    permutation: permutation_from_json(&tuple[2])?,
                }
            } else if tuple.len() == 2 {
                WireOp::ReorderShort(permutation_from_json(&tuple[1])?)
            } else {
                return Err(DeltaError::InvalidOp("m arity".to_owned()));
            }
        }
        "#" => {
            if tuple.len() != 3 || !tuple[1].is_u64() || !tuple[2].is_array() {
                return Err(DeltaError::InvalidOp("# shape".to_owned()));
            }
            let id = tuple[1].as_u64().expect("checked above");
            let path = match wire_ref_from_json(&tuple[2])? {
                PathRef::Path(path) => path,
                PathRef::Id(_) => return Err(DeltaError::InvalidOp("# shape".to_owned())),
            };
            WireOp::Define { id, path }
        }
        other => return Err(DeltaError::InvalidOp(format!("unknown op verb: {other}"))),
    };
    Ok(op)
}

fn path_key(path: &[Seg]) -> String {
    serde_json::to_string(&path_to_json(path)).expect("path JSON serialization cannot fail")
}

/// `assertPermutation` (`delta/index.ts:199-208`): an array of distinct
/// in-range indices — a bijection.
fn permutation_from_json(value: &JsonValue) -> Result<Vec<usize>, DeltaError> {
    let items = value
        .as_array()
        .ok_or_else(|| DeltaError::InvalidOp("m permutation is not an array".to_owned()))?;
    let mut seen = vec![false; items.len()];
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let index = item
            .as_f64()
            .filter(|n| n.is_finite() && n.fract() == 0.0 && *n >= 0.0 && *n < items.len() as f64)
            .ok_or_else(|| DeltaError::InvalidOp("m permutation is not a bijection".to_owned()))?
            as usize;
        if seen[index] {
            return Err(DeltaError::InvalidOp(
                "m permutation is not a bijection".to_owned(),
            ));
        }
        seen[index] = true;
        out.push(index);
    }
    Ok(out)
}

/// Stateful path-interning encoder. Port of `encoder`/`Encoder`
/// (`delta/index.ts:1527-1622`): intern on SECOND use — a definition costs
/// more than the path it replaces, so interning on first use loses on the many
/// paths written exactly once.
#[derive(Debug, Default)]
pub struct Encoder {
    seen: HashSet<String>,
    ids: HashMap<String, u64>,
    next_id: u64,
    previous: Option<String>,
}

impl Encoder {
    /// `encoder()` (`delta/index.ts:1535`).
    pub fn new() -> Encoder {
        Encoder::default()
    }

    /// `encode(ops)` (`delta/index.ts:1541-1620`).
    pub fn encode(&mut self, ops: &[Op]) -> Vec<WireOp> {
        // Arity omission is scoped to a batch: letting it span batches would
        // make a batch's first op depend on the previous batch's last one.
        self.previous = None;
        let mut out = Vec::new();
        for op in ops {
            if let Op::Replace(value) = op {
                out.push(WireOp::Replace(value.clone()));
                // A base batch is a RECOVERY POINT: everything after it must be
                // self-contained, so the dictionary resets
                // (delta/index.ts:1552-1559).
                self.seen.clear();
                self.ids.clear();
                self.next_id = 0;
                self.previous = None;
                continue;
            }
            let path = op.path().expect("non-replace op has a path");
            let key = path_key(path);

            // Same path as the previous op: drop the ref entirely.
            if self.previous.as_deref() == Some(key.as_str()) {
                out.push(match op {
                    Op::Set { value, .. } => WireOp::SetShort(value.clone()),
                    Op::Delete { .. } => WireOp::DeleteShort,
                    Op::Append { text, .. } => WireOp::AppendShort(text.clone()),
                    Op::Truncate { count, .. } => WireOp::TruncateShort(*count),
                    Op::Splice {
                        index,
                        remove,
                        items,
                        ..
                    } => WireOp::SpliceShort {
                        index: *index,
                        remove: *remove,
                        items: items.clone(),
                    },
                    Op::Reorder { permutation, .. } => WireOp::ReorderShort(permutation.clone()),
                    Op::Replace(_) => unreachable!("handled above"),
                });
                continue;
            }

            let r = if let Some(existing) = self.ids.get(&key).copied() {
                PathRef::Id(existing)
            } else if self.seen.contains(&key) {
                let id = self.next_id;
                self.next_id += 1;
                self.ids.insert(key.clone(), id);
                out.push(WireOp::Define {
                    id,
                    path: path.clone(),
                }); // second use: define, then reference
                PathRef::Id(id)
            } else {
                self.seen.insert(key.clone()); // first use: inline
                PathRef::Path(path.clone())
            };

            let wire = match op {
                Op::Set { value, .. } => WireOp::Set {
                    r,
                    value: value.clone(),
                },
                Op::Delete { .. } => WireOp::Delete { r },
                Op::Append { text, .. } => WireOp::Append {
                    r,
                    text: text.clone(),
                },
                Op::Truncate { count, .. } => WireOp::Truncate { r, count: *count },
                Op::Splice {
                    index,
                    remove,
                    items,
                    ..
                } => WireOp::Splice {
                    r,
                    index: *index,
                    remove: *remove,
                    items: items.clone(),
                },
                Op::Reorder { permutation, .. } => WireOp::Reorder {
                    r,
                    permutation: permutation.clone(),
                },
                Op::Replace(_) => unreachable!("handled above"),
            };
            out.push(wire);
            self.previous = Some(key);
        }
        out
    }
}

/// Stateful decoder. Port of `decoder`/`Decoder` (`delta/index.ts:1624-1697`).
#[derive(Debug, Default)]
pub struct Decoder {
    paths: HashMap<u64, Path>,
}

impl Decoder {
    /// `decoder()` (`delta/index.ts:1628`).
    pub fn new() -> Decoder {
        Decoder::default()
    }

    /// `decode(wire)` (`delta/index.ts:1631-1694`).
    pub fn decode(&mut self, wire: &[WireOp]) -> Result<Vec<Op>, DeltaError> {
        let mut previous: Option<Path> = None; // scoped to the batch, as in encode
        let mut out = Vec::new();
        for op in wire {
            match op {
                WireOp::Define { id, path } => {
                    assert_safe_path(path)?;
                    self.paths.insert(*id, path.clone());
                }
                WireOp::Replace(value) => {
                    out.push(Op::Replace(value.clone()));
                    self.paths.clear();
                    previous = None;
                }
                _ => {
                    // Arity tells us whether a ref is present: the short forms
                    // omit it.
                    let short = matches!(
                        op,
                        WireOp::DeleteShort
                            | WireOp::SetShort(_)
                            | WireOp::AppendShort(_)
                            | WireOp::TruncateShort(_)
                            | WireOp::SpliceShort { .. }
                            | WireOp::ReorderShort(_)
                    );

                    let path: Path = if short {
                        let Some(previous) = &previous else {
                            return Err(unresolvable(&[]));
                        };
                        previous.clone()
                    } else {
                        let r = match op {
                            WireOp::Set { r, .. }
                            | WireOp::Delete { r }
                            | WireOp::Append { r, .. }
                            | WireOp::Truncate { r, .. }
                            | WireOp::Splice { r, .. }
                            | WireOp::Reorder { r, .. } => r,
                            _ => unreachable!("covered above"),
                        };
                        let path = match r {
                            PathRef::Id(id) => self.paths.get(id).cloned().ok_or_else(|| {
                                DeltaError::UnresolvablePath {
                                    path: id.to_string(),
                                }
                            })?,
                            PathRef::Path(path) => path.clone(),
                        };
                        previous = Some(path.clone());
                        path
                    };

                    if !matches!(
                        op,
                        WireOp::Splice { .. }
                            | WireOp::SpliceShort { .. }
                            | WireOp::Reorder { .. }
                            | WireOp::ReorderShort(_)
                    ) && path.is_empty()
                    {
                        return Err(unresolvable(&path));
                    }
                    match op {
                        WireOp::Set { value, .. } => out.push(Op::Set {
                            path,
                            value: value.clone(),
                        }),
                        WireOp::SetShort(value) => out.push(Op::Set {
                            path,
                            value: value.clone(),
                        }),
                        WireOp::Delete { .. } | WireOp::DeleteShort => {
                            out.push(Op::Delete { path })
                        }
                        WireOp::Append { text, .. } | WireOp::AppendShort(text) => {
                            out.push(Op::Append {
                                path,
                                text: text.clone(),
                            })
                        }
                        WireOp::Truncate { count, .. } | WireOp::TruncateShort(count) => {
                            out.push(Op::Truncate {
                                path,
                                count: *count,
                            })
                        }
                        WireOp::Splice {
                            index,
                            remove,
                            items,
                            ..
                        }
                        | WireOp::SpliceShort {
                            index,
                            remove,
                            items,
                        } => out.push(Op::Splice {
                            path,
                            index: *index,
                            remove: *remove,
                            items: items.clone(),
                        }),
                        WireOp::Reorder { permutation, .. } | WireOp::ReorderShort(permutation) => {
                            out.push(Op::Reorder {
                                path,
                                permutation: permutation.clone(),
                            })
                        }
                        _ => unreachable!("define/replace handled above"),
                    }
                }
            }
        }
        Ok(out)
    }
}

/// Convenience constructor pair used by callers that keep an encoder/decoder
/// next to each other (`encoder()` / `decoder()` upstream).
pub fn encoder() -> Encoder {
    Encoder::new()
}

/// `decoder()` (`delta/index.ts:1628`).
pub fn decoder() -> Decoder {
    Decoder::new()
}
