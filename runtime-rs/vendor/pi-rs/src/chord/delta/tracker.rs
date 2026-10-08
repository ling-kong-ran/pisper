//! Port of `packages/chord/src/delta/tracker.ts` (upstream sha256
//! `3be4c18b42a4fff288617d96f8547137a485b41b0f455c2c811beae5b4bc5778`): the
//! overlay transaction tracker behind `track()` — `beginChange()` drafts,
//! `prepare()`/`abort()`, `prepareReplace()` and `adopt()`.
//!
//! # Draft surface (disclosed divergence, continuing M6/D2)
//!
//! Upstream drafts are JS `Proxy` graphs: writes land in per-node overlay
//! records (single-slot `writeKey` spilling into an insertion-ordered
//! `writes` map, `deletes`, `readded`), arrays keep a piece table with
//! base-entry overrides and insert-source overrides, and every read composes
//! base + overlay through proxy traps. The port reproduces that machinery
//! over an explicit path-addressed mutation API — [`Change::set`],
//! [`Change::delete`], [`Change::push`]/[`pop`]/[`shift`]/[`unshift`]/
//! [`splice`]/[`set_length`]/[`reverse`]/[`sort_with`]/[`fill`]/
//! [`copy_within`] — each mirroring the observable behavior of one upstream
//! trap or array mutator (which overlay record it writes, which ops
//! `prepare()` then emits). Reads ([`Change::read`]) resolve base + overlay
//! the way the traps do but return plain values (no live proxies).
//!
//! JS container identity, which upstream uses for "is this position still
//! the container my node wraps" checks, is stand-in modeled with stored-slot
//! epochs: a fresh slot is a fresh object and replacing a slot's content
//! bumps its epoch (upstream `replaceStoredValue` writes a different
//! object). Containers compared by identity upstream never compare equal
//! here — exactly upstream's behavior for the alias-free inputs `track()`
//! documents; wrapper identity caching and structural aliasing of one
//! container at several positions are otherwise unrepresentable (each
//! position gets its own node). `sort` comparators receive resolved plain
//! values (upstream hands proxies), so mutating comparators and the
//! structural-comparator deduplicate pass are unreachable. The WeakRef/GC
//! registry is invisible. Every value, op, revision and error message the
//! upstream public surface produces is preserved; see
//! `tests/fixtures/chord_delta_oracle/`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use super::apply_immutable_trusted::apply_immutable_trusted;
use super::{
    is_reserved, overlap, slice_utf16_from, utf16_len, DeltaError, JsonValue, Op, Path, Seg,
};

const MAX_DELTA_OPERATIONS: usize = 4_096;
const MAX_SIMPLE_OBJECT_NODES: usize = 128;
const DENSE_CANDIDATE_BITS: usize = 256;
const DENSE_REGION_MIN_COUNT: usize = 256;

// ─── Public surface ──────────────────────────────────────────────────────────

/// Port of `Prepared<T>` (`tracker.ts:121-127`): one prepared immutable
/// revision with the operations that produce it from [`Prepared::base`].
pub struct Prepared {
    core: Arc<Mutex<TrackerCore>>,
    context: usize,
    owner: u64,
    /// Upstream `base` (`tracker.ts:122`): the revision prepared against.
    pub base: JsonValue,
    /// Upstream `value` (`tracker.ts:123`): the prepared immutable revision.
    pub value: JsonValue,
    /// Upstream `ops` (`tracker.ts:124`): the batch from `base` to `value`.
    pub ops: Vec<Op>,
    /// Upstream `baseRevision` (`tracker.ts:125-126`).
    pub base_revision: u64,
}

impl Clone for Prepared {
    fn clone(&self) -> Prepared {
        Prepared {
            core: Arc::clone(&self.core),
            context: self.context,
            owner: self.owner,
            base: self.base.clone(),
            value: self.value.clone(),
            ops: self.ops.clone(),
            base_revision: self.base_revision,
        }
    }
}

impl std::fmt::Debug for Prepared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Prepared")
            .field("base_revision", &self.base_revision)
            .field("ops", &self.ops.len())
            .finish()
    }
}

impl Prepared {
    /// `prepared.abort()` (`tracker.ts:161-163`): discard the prepared
    /// revision unless it was already adopted.
    pub fn abort(self) {
        let mut core = self.lock();
        abort_context(&mut core, self.context);
    }

    fn lock(&self) -> MutexGuard<'_, TrackerCore> {
        self.core
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Port of `Change<T>` (`tracker.ts:129-133`): one open transaction. The
/// port's `Change` handle *is* the draft (upstream `change.state`):
/// mutations go through the path-addressed methods below.
pub struct Change {
    core: Arc<Mutex<TrackerCore>>,
    context: usize,
    settled: bool,
}

impl std::fmt::Debug for Change {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Change").finish()
    }
}

impl Change {
    /// Resolve `path` against the draft (base + overlay). Port of the `get`
    /// trap chain (`getProperty`, `tracker.ts:524-561`); `None` is upstream
    /// `undefined`.
    pub fn read(&self, path: &[Seg]) -> Result<Option<JsonValue>, TrackerError> {
        if self.settled {
            return Err(error(TrackerErrorKind::Plain(
                "Change has already been settled".to_owned(),
            )));
        }
        let mut core = self.lock();
        assert_readable(&core, self.context)?;
        let mut failure = None;
        match walk(&mut core, self.context, path, &mut failure)? {
            Some(node) => Ok(Some(clone_node(&core, self.context, node))),
            None => Ok(None),
        }
    }

    /// The `set` trap (`setProperty`, `tracker.ts:563-591`) at `path`.
    pub fn set(&self, path: &[Seg], value: JsonValue) -> Result<(), TrackerError> {
        if self.settled {
            return Err(error(TrackerErrorKind::Plain(
                "Change has already been settled".to_owned(),
            )));
        }
        let mut core = self.lock();
        assert_writable(&core, self.context)?;
        let (parent, key) = split_path(path)?;
        let mut failure = None;
        let node = walk(&mut core, self.context, parent, &mut failure)?.ok_or_else(|| {
            match failure {
                // The draft read resolved to a primitive; the JS runtime
                // message names its type (`Cannot create property 'b' on
                // number '1'`).
                Some(WalkFailure::Primitive(primitive)) => type_error(format!(
                    "Cannot create property '{key}' on {} '{}'",
                    js_typeof(&primitive),
                    js_value_to_string(&primitive)
                )),
                Some(WalkFailure::Null) => {
                    type_error(format!("Cannot set properties of null (setting '{key}')"))
                }
                _ => type_error(format!(
                    "Cannot set properties of undefined (setting '{key}')"
                )),
            }
        })?;
        set_property(&mut core, self.context, node, key, value)
    }

    /// The `deleteProperty` trap (`tracker.ts:593-604`).
    pub fn delete(&self, path: &[Seg]) -> Result<(), TrackerError> {
        if self.settled {
            return Err(error(TrackerErrorKind::Plain(
                "Change has already been settled".to_owned(),
            )));
        }
        let mut core = self.lock();
        assert_writable(&core, self.context)?;
        let (parent, key) = split_path(path)?;
        let mut failure = None;
        let node =
            walk(&mut core, self.context, parent, &mut failure)?.ok_or_else(|| match failure {
                Some(WalkFailure::Primitive(primitive)) => type_error(format!(
                    "Cannot convert {} '{}' to object (deleting '{key}')",
                    js_typeof(&primitive),
                    js_value_to_string(&primitive)
                )),
                _ => type_error(format!(
                    "Cannot read properties of undefined (reading '{key}')"
                )),
            })?;
        if is_array_base(&core, self.context, node) {
            return Err(type_error("Overlay arrays cannot contain holes"));
        }
        let Seg::Key(key) = key else {
            return Err(type_error("Overlay arrays cannot contain holes"));
        };
        delete_property(&mut core, self.context, node, key)
    }

    fn mutator_node(&self, core: &mut TrackerCore, path: &[Seg]) -> Result<usize, TrackerError> {
        let mut failure = None;
        let node = walk(core, self.context, path, &mut failure)?
            .ok_or_else(|| type_error("Array mutator called on incompatible receiver"))?;
        if !is_array_base(core, self.context, node) {
            return Err(type_error("Array mutator called on incompatible receiver"));
        }
        Ok(node)
    }

    /// `push` (`tracker.ts:1178-1184`); returns the new length.
    pub fn push(&self, path: &[Seg], items: Vec<JsonValue>) -> Result<usize, TrackerError> {
        if self.settled {
            return Err(error(TrackerErrorKind::Plain(
                "Change has already been settled".to_owned(),
            )));
        }
        let mut core = self.lock();
        assert_writable(&core, self.context)?;
        let node = self.mutator_node(&mut core, path)?;
        let context = self.context;
        let pieces = insert_pieces(&mut core, context, node, items);
        let length = array_length_at(&core, context, node);
        replace_piece_range(&mut core, context, node, length, 0, pieces);
        Ok(array_length_at(&core, context, node))
    }

    /// `pop` (`tracker.ts:1185-1193`).
    pub fn pop(&self, path: &[Seg]) -> Result<Option<JsonValue>, TrackerError> {
        if self.settled {
            return Err(error(TrackerErrorKind::Plain(
                "Change has already been settled".to_owned(),
            )));
        }
        let mut core = self.lock();
        assert_writable(&core, self.context)?;
        let node = self.mutator_node(&mut core, path)?;
        let context = self.context;
        let length = array_length_at(&core, context, node);
        if length == 0 {
            return Ok(None);
        }
        let value = get_array_index_value(&core, context, node, length - 1);
        replace_piece_range(&mut core, context, node, length - 1, 1, Vec::new());
        Ok(value)
    }

    /// `shift` (`tracker.ts:1194-1202`).
    pub fn shift(&self, path: &[Seg]) -> Result<Option<JsonValue>, TrackerError> {
        if self.settled {
            return Err(error(TrackerErrorKind::Plain(
                "Change has already been settled".to_owned(),
            )));
        }
        let mut core = self.lock();
        assert_writable(&core, self.context)?;
        let node = self.mutator_node(&mut core, path)?;
        let context = self.context;
        let length = array_length_at(&core, context, node);
        if length == 0 {
            return Ok(None);
        }
        let value = get_array_index_value(&core, context, node, 0);
        replace_piece_range(&mut core, context, node, 0, 1, Vec::new());
        Ok(value)
    }

    /// `unshift` (`tracker.ts:1203-1208`).
    pub fn unshift(&self, path: &[Seg], items: Vec<JsonValue>) -> Result<usize, TrackerError> {
        if self.settled {
            return Err(error(TrackerErrorKind::Plain(
                "Change has already been settled".to_owned(),
            )));
        }
        let mut core = self.lock();
        assert_writable(&core, self.context)?;
        let node = self.mutator_node(&mut core, path)?;
        let context = self.context;
        let pieces = insert_pieces(&mut core, context, node, items);
        replace_piece_range(&mut core, context, node, 0, 0, pieces);
        Ok(array_length_at(&core, context, node))
    }

    /// `splice` (`tracker.ts:1209-1226`) with upstream argument
    /// normalization: negative `start` counts from the tail, `remove`
    /// clamps to `[0, length - start]`.
    pub fn splice(
        &self,
        path: &[Seg],
        start: isize,
        remove: isize,
        items: Vec<JsonValue>,
    ) -> Result<Vec<JsonValue>, TrackerError> {
        if self.settled {
            return Err(error(TrackerErrorKind::Plain(
                "Change has already been settled".to_owned(),
            )));
        }
        let mut core = self.lock();
        assert_writable(&core, self.context)?;
        let node = self.mutator_node(&mut core, path)?;
        let context = self.context;
        let length = array_length_at(&core, context, node);
        let start = clamp_index(start, length);
        let remove = if remove < 0 {
            0
        } else {
            remove.min((length - start) as isize) as usize
        };
        let removed: Vec<JsonValue> = (0..remove)
            .map(|offset| {
                get_array_index_value(&core, context, node, start + offset)
                    .unwrap_or(JsonValue::Null)
            })
            .collect();
        // `insertPlacementPiece(node, args, 2)` over the JS arguments list:
        // the two leading argument slots remain primitive junk refs and the
        // piece starts at offset 2 inside its source.
        let item_count = items.len();
        let pieces = splice_pieces(&mut core, context, start, remove, items);
        replace_piece_range(&mut core, context, node, start, remove, pieces);
        set_array_length(&mut core, context, node, length - remove + item_count);
        Ok(removed)
    }

    /// `sort` with the default JS comparator (elements ordered by their
    /// UTF-16 string form) (`tracker.ts:1244-1313`, default branch).
    pub fn sort_default(&self, path: &[Seg]) -> Result<(), TrackerError> {
        self.sort_with(path, |left, right| {
            let a = js_value_to_string(left);
            let b = js_value_to_string(right);
            // JS `<`/`>` compare UTF-16 code units, not UTF-8 bytes.
            let a_units = a.encode_utf16().collect::<Vec<_>>();
            let b_units = b.encode_utf16().collect::<Vec<_>>();
            match a_units.cmp(&b_units) {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Greater => 1,
                std::cmp::Ordering::Equal => 0,
            }
        })
    }

    /// `sort` with a caller comparator (`tracker.ts:1244-1313`). The
    /// comparator receives resolved plain values (upstream hands proxies;
    /// mutating comparators are not representable here).
    pub fn sort_with(
        &self,
        path: &[Seg],
        mut comparator: impl FnMut(&JsonValue, &JsonValue) -> i32,
    ) -> Result<(), TrackerError> {
        if self.settled {
            return Err(error(TrackerErrorKind::Plain(
                "Change has already been settled".to_owned(),
            )));
        }
        let mut core = self.lock();
        assert_writable(&core, self.context)?;
        let node = self.mutator_node(&mut core, path)?;
        sort_array(&mut core, self.context, node, &mut comparator);
        Ok(())
    }

    /// `reverse` (`tracker.ts:1227-1243`).
    pub fn reverse(&self, path: &[Seg]) -> Result<(), TrackerError> {
        if self.settled {
            return Err(error(TrackerErrorKind::Plain(
                "Change has already been settled".to_owned(),
            )));
        }
        let mut core = self.lock();
        assert_writable(&core, self.context)?;
        let node = self.mutator_node(&mut core, path)?;
        reverse_array(&mut core, self.context, node);
        Ok(())
    }

    /// `fill` (`tracker.ts:1314-1324`).
    pub fn fill(
        &self,
        path: &[Seg],
        supplied: JsonValue,
        start: Option<isize>,
        end: Option<isize>,
    ) -> Result<(), TrackerError> {
        if self.settled {
            return Err(error(TrackerErrorKind::Plain(
                "Change has already been settled".to_owned(),
            )));
        }
        let mut core = self.lock();
        assert_writable(&core, self.context)?;
        let node = self.mutator_node(&mut core, path)?;
        let context = self.context;
        let length = array_length_at(&core, context, node);
        let start = match start {
            Some(at) => clamp_index(at, length),
            None => 0,
        };
        let end = match end {
            Some(at) => clamp_index(at, length),
            None => length,
        };
        if end <= start {
            return Ok(());
        }
        let items: Vec<JsonValue> = (0..end - start).map(|_| supplied.clone()).collect();
        let pieces = insert_pieces(&mut core, context, node, items);
        replace_piece_range(&mut core, context, node, start, end - start, pieces);
        Ok(())
    }

    /// `copyWithin` (`tracker.ts:1325-1336`).
    pub fn copy_within(
        &self,
        path: &[Seg],
        target: isize,
        start: isize,
        end: Option<isize>,
    ) -> Result<(), TrackerError> {
        if self.settled {
            return Err(error(TrackerErrorKind::Plain(
                "Change has already been settled".to_owned(),
            )));
        }
        let mut core = self.lock();
        assert_writable(&core, self.context)?;
        let node = self.mutator_node(&mut core, path)?;
        let context = self.context;
        let length = array_length_at(&core, context, node);
        let target = clamp_index(target, length);
        let start = clamp_index(start, length);
        let end = match end {
            Some(at) => clamp_index(at, length),
            None => length,
        };
        let count = end.saturating_sub(start).min(length - target);
        let values: Vec<JsonValue> = (0..count)
            .map(|offset| {
                get_array_index_value(&core, context, node, start + offset)
                    .unwrap_or(JsonValue::Null)
            })
            .collect();
        let pieces = insert_pieces(&mut core, context, node, values);
        replace_piece_range(&mut core, context, node, target, count, pieces);
        Ok(())
    }

    /// The `length` set trap (`setArrayLength` via `setProperty`,
    /// `tracker.ts:566-569, 1134-1148`).
    pub fn set_length(&self, path: &[Seg], next: usize) -> Result<(), TrackerError> {
        if self.settled {
            return Err(error(TrackerErrorKind::Plain(
                "Change has already been settled".to_owned(),
            )));
        }
        let mut core = self.lock();
        assert_writable(&core, self.context)?;
        let mut failure = None;
        let node = walk(&mut core, self.context, path, &mut failure)?
            .ok_or_else(|| type_error("Only array indices and length can be written"))?;
        if !is_array_base(&core, self.context, node) {
            return Err(type_error("Only array indices and length can be written"));
        }
        set_array_length(&mut core, self.context, node, next);
        Ok(())
    }

    /// `change.prepare()` (`tracker.ts:181-200`): freeze the draft, emit
    /// the operation batch and materialize the resulting revision.
    /// Consumes the change; the context aborts if emission fails.
    pub fn prepare(mut self) -> Result<Prepared, TrackerError> {
        if self.settled {
            return Err(error(TrackerErrorKind::Plain(
                "Change has already been settled".to_owned(),
            )));
        }
        self.settled = true;
        let core = Arc::clone(&self.core);
        let mut guard = self.lock();
        let context = self.context;
        assert_writable(&guard, context)?;
        guard.contexts[context].status = Status::Prepared;
        match prepare_context(&mut guard, context) {
            Ok((base, value, ops)) => Ok(Prepared {
                core,
                context,
                owner: guard.id,
                base,
                value,
                ops,
                base_revision: guard.contexts[context].base_revision,
            }),
            Err(report) => {
                guard.contexts[context].status = Status::Aborted;
                clear_context(&mut guard, context);
                Err(report)
            }
        }
    }

    /// `change.abort()` (`tracker.ts:202-211`).
    pub fn abort(mut self) {
        if self.settled {
            return;
        }
        self.settled = true;
        let mut core = self.lock();
        abort_context(&mut core, self.context);
    }

    fn lock(&self) -> MutexGuard<'_, TrackerCore> {
        self.core
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Port of `Tracker<T>` (`tracker.ts:135-141`).
#[derive(Clone)]
pub struct Tracker {
    core: Arc<Mutex<TrackerCore>>,
}

impl std::fmt::Debug for Tracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tracker")
            .field("revision", &self.revision())
            .finish()
    }
}

static NEXT_TRACKER_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl Tracker {
    /// `get value` (`tracker.ts:227-229`).
    pub fn value(&self) -> JsonValue {
        self.lock().value.clone()
    }

    /// `get revision` (`tracker.ts:231-233`).
    pub fn revision(&self) -> u64 {
        self.lock().revision
    }

    /// `beginChange()` (`tracker.ts:235-239`).
    pub fn begin_change(&self) -> Change {
        let mut core = self.lock();
        let (base_revision, value) = (core.revision, core.value.clone());
        let context = core.create_context(false, value.clone(), value, base_revision);
        Change {
            core: Arc::clone(&self.core),
            context,
            settled: false,
        }
    }

    /// `prepareReplace(value)` (`tracker.ts:241-252`).
    pub fn prepare_replace(&self, value: JsonValue) -> Result<Prepared, TrackerError> {
        let core_arc = Arc::clone(&self.core);
        let mut core = self.lock();
        let (base_revision, base_value) = (core.revision, core.value.clone());
        let context = core.create_context(true, value, base_value, base_revision);
        core.contexts[context].status = Status::Prepared;
        match prepare_context(&mut core, context) {
            Ok((base, value, ops)) => Ok(Prepared {
                core: core_arc,
                context,
                owner: core.id,
                base,
                value,
                ops,
                base_revision: core.contexts[context].base_revision,
            }),
            Err(report) => {
                core.contexts[context].status = Status::Aborted;
                clear_context(&mut core, context);
                Err(report)
            }
        }
    }

    /// `adopt(prepared)` (`tracker.ts:254-277`): an infallible pointer swap
    /// upstream; the checks below reproduce its failure contract.
    pub fn adopt(&self, prepared: Prepared) -> Result<(), TrackerError> {
        let mut core = self.lock();
        if prepared.owner != core.id {
            return Err(error(TrackerErrorKind::Plain(
                "Prepared change belongs to a different tracker".to_owned(),
            )));
        }
        let context = prepared.context;
        match core.contexts[context].status {
            Status::Consumed => {
                return Err(error(TrackerErrorKind::Plain(
                    "Prepared change has already been used".to_owned(),
                )))
            }
            Status::Aborted => {
                return Err(error(TrackerErrorKind::Plain(
                    "Prepared change has been aborted".to_owned(),
                )))
            }
            Status::Stale => {
                return Err(error(TrackerErrorKind::Plain(
                    "Prepared change is stale".to_owned(),
                )))
            }
            Status::Open => {
                return Err(error(TrackerErrorKind::Plain(
                    "Prepared change is not ready".to_owned(),
                )))
            }
            Status::Prepared => {}
        }
        if core.contexts[context].base_revision != core.revision {
            core.contexts[context].status = Status::Stale;
            clear_context(&mut core, context);
            return Err(error(TrackerErrorKind::Plain(
                "Prepared change is stale".to_owned(),
            )));
        }
        // Upstream also rejects when `this.#value !== prepared.base` by
        // identity; over owned trees that condition is unreachable once the
        // base revision matches (see module docs).
        core.value = prepared.value;
        core.contexts[context].status = Status::Consumed;
        core.revision += 1;
        invalidate(&mut core, context);
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, TrackerCore> {
        self.core
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// `track(initial)` (`tracker.ts:320-323`): take immutable ownership of an
/// alias-free strict-JSON root in O(1). The caller must not mutate
/// `initial` afterwards (an ownership contract).
pub fn track(initial: JsonValue) -> Tracker {
    let id = NEXT_TRACKER_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Tracker {
        core: Arc::new(Mutex::new(TrackerCore {
            id,
            value: initial,
            revision: 0,
            contexts: Vec::new(),
        })),
    }
}

// ─── Error taxonomy ──────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TrackerErrorKind {
    /// Upstream `TypeError`.
    Type(String),
    /// Upstream `Error`.
    Plain(String),
}

/// Error raised by the tracker surface; [`TrackerError::message`]
/// reproduces the upstream error `message` byte-for-byte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackerError(pub(crate) TrackerErrorKind);

impl TrackerError {
    /// The upstream `error.message` text.
    pub fn message(&self) -> String {
        match &self.0 {
            TrackerErrorKind::Type(message) | TrackerErrorKind::Plain(message) => message.clone(),
        }
    }

    /// Upstream error class name.
    pub fn kind(&self) -> &'static str {
        match &self.0 {
            TrackerErrorKind::Type(_) => "TypeError",
            TrackerErrorKind::Plain(_) => "Error",
        }
    }

    /// Route a tracker failure into the shared [`DeltaError`] taxonomy.
    pub fn into_delta(self) -> DeltaError {
        DeltaError::InvalidOp(self.message())
    }
}

impl std::fmt::Display for TrackerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for TrackerError {}

impl From<DeltaError> for TrackerError {
    fn from(report: DeltaError) -> TrackerError {
        TrackerError(TrackerErrorKind::Type(report.message()))
    }
}

fn type_error(message: impl Into<String>) -> TrackerError {
    TrackerError(TrackerErrorKind::Type(message.into()))
}

fn error(kind: TrackerErrorKind) -> TrackerError {
    TrackerError(kind)
}

// ─── Core state ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Open,
    Prepared,
    Consumed,
    Aborted,
    Stale,
}

/// What the parent's position pointed at when a node was created. Upstream
/// compares container identity; owned trees stand in with a slot/epoch pair
/// (same slot + epoch means the same stored container).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Origin {
    /// The parent's base value at the position (no overriding write).
    Base,
    /// A stored slot as of a particular write epoch.
    Slot { slot: usize, epoch: u64 },
    /// A scalar or deletion displaced the position; no container matches.
    Scalar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParentKind {
    Object,
    BaseEntry,
    InsertEntry,
}

#[derive(Clone, Debug)]
enum ParentKey {
    Key(String),
    Source(usize),
}

#[derive(Clone, Debug)]
enum Piece {
    Base {
        start: usize,
        length: usize,
        step: i64,
    },
    Insert {
        source: u64,
        start: usize,
        length: usize,
        step: i64,
    },
}

impl Piece {
    fn length(&self) -> usize {
        match self {
            Piece::Base { length, .. } | Piece::Insert { length, .. } => *length,
        }
    }

    fn start(&self) -> usize {
        match self {
            Piece::Base { start, .. } | Piece::Insert { start, .. } => *start,
        }
    }

    fn step(&self) -> i64 {
        match self {
            Piece::Base { step, .. } | Piece::Insert { step, .. } => *step,
        }
    }

    fn set_length(&mut self, length: usize) {
        match self {
            Piece::Base { length: at, .. } | Piece::Insert { length: at, .. } => *at = length,
        }
    }

    fn set_start(&mut self, start: usize) {
        match self {
            Piece::Base { start: at, .. } | Piece::Insert { start: at, .. } => *at = start,
        }
    }

    fn set_step(&mut self, step: i64) {
        match self {
            Piece::Base { step: at, .. } | Piece::Insert { step: at, .. } => *at = step,
        }
    }

    fn is_base(&self) -> bool {
        matches!(self, Piece::Base { .. })
    }
}

/// An array-entry reference: base content, an inline primitive, or a stored
/// slot at a write epoch (`entryValueAt`, `tracker.ts:986-997`).
#[derive(Clone, Debug)]
enum EntryRef {
    Base,
    Primitive(JsonValue),
    Slot { slot: usize, epoch: u64 },
}

impl EntryRef {
    fn from(&self) -> Origin {
        match self {
            EntryRef::Base => Origin::Base,
            EntryRef::Primitive(_) => Origin::Scalar,
            EntryRef::Slot { slot, epoch } => Origin::Slot {
                slot: *slot,
                epoch: *epoch,
            },
        }
    }
}

/// A stored write value: primitives inline (JS value-identity over them is
/// value equality), containers in the slot arena.
#[derive(Clone, Debug)]
enum WriteValue {
    Primitive(JsonValue),
    Slot { slot: usize, epoch: u64 },
}

impl WriteValue {
    fn from(&self) -> Origin {
        match self {
            WriteValue::Primitive(_) => Origin::Scalar,
            WriteValue::Slot { slot, epoch } => Origin::Slot {
                slot: *slot,
                epoch: *epoch,
            },
        }
    }
}

struct ArrayPlan {
    remove_runs: Vec<usize>,
    permutation: Option<Vec<usize>>,
    insert_runs: Vec<usize>,
}

#[derive(Default)]
struct ArrayOverlay {
    pieces: Vec<Piece>,
    /// Insertion-ordered `baseOverrides` (`tracker.ts:45`).
    base_overrides: Vec<(usize, EntryRef)>,
    /// Insertion-ordered `insertOverrides` (`tracker.ts:46`): source id →
    /// ordered (source index → ref).
    insert_overrides: Vec<(u64, Vec<(usize, EntryRef)>)>,
    structural: bool,
    generation: u64,
    plan: Option<ArrayPlan>,
}

impl ArrayOverlay {
    /// `tracker.ts:784-785`: a fresh overlay starts with one base piece
    /// covering the whole base array.
    fn for_base(base: &JsonValue) -> ArrayOverlay {
        let mut overlay = ArrayOverlay::default();
        if let Some(items) = base.as_array() {
            if !items.is_empty() {
                overlay.pieces = vec![Piece::Base {
                    start: 0,
                    length: items.len(),
                    step: 1,
                }];
            }
        }
        overlay
    }
}

struct OverlayNode {
    base: JsonValue,
    parent: Option<usize>,
    parent_kind: ParentKind,
    parent_key: ParentKey,
    parent_source: Option<u64>,
    parent_placement: bool,
    from: Origin,
    write_key: Option<String>,
    write_value: Option<WriteValue>,
    writes: Vec<(String, WriteValue)>,
    delete_key: Option<String>,
    deletes: Vec<String>,
    readded: Vec<String>,
    array: ArrayOverlay,
    dirty: bool,
    subtree_dirty: bool,
    /// Child nodes created through reads at object keys.
    child_keys: HashMap<String, usize>,
    /// Child nodes created through array entries:
    /// (is-base-entry, source id, source index) → node.
    child_entries: HashMap<(bool, u64, usize), usize>,
}

struct Overlay {
    root: usize,
    nodes: Vec<OverlayNode>,
    stored: Vec<Option<JsonValue>>,
    epochs: Vec<u64>,
    next_epoch: u64,
    next_source: u64,
    /// Insert sources (`InsertSource.refs`): `None` once merged away.
    sources: Vec<Option<Vec<EntryRef>>>,
    dirty: Vec<usize>,
    replacement: bool,
    replacement_noop: bool,
    base_value: JsonValue,
    simple_object_materialization: bool,
    ops: Option<Vec<Op>>,
}

impl Overlay {
    /// `storeValue` (`tracker.ts:473-478`): primitives inline, containers
    /// in fresh slots.
    fn store(&mut self, value: JsonValue) -> EntryRef {
        if value.is_object() || value.is_array() {
            let slot = self.stored.len();
            self.stored.push(Some(value));
            self.epochs.push(self.next_epoch);
            self.next_epoch += 1;
            EntryRef::Slot {
                slot,
                epoch: self.next_epoch - 1,
            }
        } else {
            EntryRef::Primitive(value)
        }
    }

    /// `replaceStoredValue` (`tracker.ts:484-494`): replace slot content in
    /// place (a different object upstream → epoch bump).
    fn replace_slot(&mut self, slot: usize, value: JsonValue) -> EntryRef {
        self.stored[slot] = Some(value);
        self.epochs[slot] = self.next_epoch;
        self.next_epoch += 1;
        EntryRef::Slot {
            slot,
            epoch: self.next_epoch - 1,
        }
    }

    /// `releaseStoredValue` (`tracker.ts:496-498`).
    fn release_slot(&mut self, slot: usize) {
        self.stored[slot] = None;
        self.epochs[slot] = self.next_epoch;
        self.next_epoch += 1;
    }

    fn slot_value(&self, slot: usize) -> JsonValue {
        self.stored[slot].clone().unwrap_or(JsonValue::Null)
    }

    /// `replaceStoredValue` semantics for a position with an optional
    /// existing slot: container values reuse the slot (epoch bump),
    /// scalars release it.
    fn overwrite(&mut self, existing_slot: Option<usize>, value: JsonValue) -> EntryRef {
        let is_container = value.is_object() || value.is_array();
        match (existing_slot, is_container) {
            (Some(slot), true) => self.replace_slot(slot, value),
            (Some(slot), false) => {
                self.release_slot(slot);
                EntryRef::Primitive(value)
            }
            (None, _) => self.store(value),
        }
    }
}

struct ContextCell {
    status: Status,
    base_revision: u64,
    overlay: Option<Overlay>,
}

struct TrackerCore {
    id: u64,
    value: JsonValue,
    revision: u64,
    contexts: Vec<ContextCell>,
}

impl TrackerCore {
    /// `createContext` (`tracker.ts:351-380`).
    fn create_context(
        &mut self,
        replacement: bool,
        root: JsonValue,
        base: JsonValue,
        base_revision: u64,
    ) -> usize {
        let mut overlay = Overlay {
            root: 0,
            nodes: Vec::new(),
            stored: Vec::new(),
            epochs: Vec::new(),
            next_epoch: 0,
            next_source: 0,
            sources: Vec::new(),
            dirty: Vec::new(),
            replacement,
            replacement_noop: false,
            base_value: base,
            simple_object_materialization: false,
            ops: None,
        };
        let root_node = create_node(
            &mut overlay,
            root,
            None,
            ParentKind::Object,
            ParentKey::Key(String::new()),
            None,
            false,
            Origin::Base,
        );
        overlay.root = root_node;
        self.contexts.push(ContextCell {
            status: Status::Open,
            base_revision,
            overlay: Some(overlay),
        });
        self.contexts.len() - 1
    }
}

fn live(core: &TrackerCore, context: usize) -> Result<&Overlay, TrackerError> {
    core.contexts
        .get(context)
        .and_then(|cell| cell.overlay.as_ref())
        .ok_or_else(|| type_error("Cannot use a settled overlay"))
}

fn live_mut(core: &mut TrackerCore, context: usize) -> Result<&mut Overlay, TrackerError> {
    core.contexts
        .get_mut(context)
        .and_then(|cell| cell.overlay.as_mut())
        .ok_or_else(|| type_error("Cannot use a settled overlay"))
}

/// `isSettledContext` (`tracker.ts:506-513`).
fn is_settled(core: &TrackerCore, context: usize) -> bool {
    match core.contexts.get(context) {
        Some(cell) => {
            matches!(
                cell.status,
                Status::Consumed | Status::Aborted | Status::Stale
            ) || cell.overlay.is_none()
        }
        None => true,
    }
}

/// `assertReadable` (`tracker.ts:515-517`).
fn assert_readable(core: &TrackerCore, context: usize) -> Result<(), TrackerError> {
    if is_settled(core, context) {
        return Err(type_error("Cannot use a settled overlay"));
    }
    Ok(())
}

/// `assertWritable` (`tracker.ts:519-522`).
fn assert_writable(core: &TrackerCore, context: usize) -> Result<(), TrackerError> {
    assert_readable(core, context)?;
    if core.contexts[context].status != Status::Open {
        return Err(type_error("Prepared overlays are read-only"));
    }
    Ok(())
}

fn is_array_base(core: &TrackerCore, context: usize, node: usize) -> bool {
    live(core, context)
        .map(|overlay| overlay.nodes[node].base.is_array())
        .unwrap_or(false)
}

// ─── Node creation and path walking ──────────────────────────────────────────

/// `createNode` (`tracker.ts:432-463`) over per-position nodes.
#[allow(clippy::too_many_arguments)]
fn create_node(
    overlay: &mut Overlay,
    base: JsonValue,
    parent: Option<usize>,
    parent_kind: ParentKind,
    parent_key: ParentKey,
    parent_source: Option<u64>,
    parent_placement: bool,
    from: Origin,
) -> usize {
    overlay.nodes.push(OverlayNode {
        array: ArrayOverlay::for_base(&base),
        base,
        parent,
        parent_kind,
        parent_key,
        parent_source,
        parent_placement,
        from,
        write_key: None,
        write_value: None,
        writes: Vec::new(),
        delete_key: None,
        deletes: Vec::new(),
        readded: Vec::new(),
        dirty: false,
        subtree_dirty: false,
        child_keys: HashMap::new(),
        child_entries: HashMap::new(),
    });
    overlay.nodes.len() - 1
}

/// Walk `path` from the overlay root, creating nodes for containers reached
/// along the way (the `getProperty`/`getArrayIndex` + `createNode` chain).
/// `None` where an intermediate does not resolve to a container, with
/// `failure` describing what the walk landed on (mirrors the JS values the
/// proxy reads return, which drive the runtime `TypeError` messages).
pub(crate) enum WalkFailure {
    Undefined,
    Null,
    Primitive(JsonValue),
}

fn walk(
    core: &mut TrackerCore,
    context: usize,
    path: &[Seg],
    failure: &mut Option<WalkFailure>,
) -> Result<Option<usize>, TrackerError> {
    let mut current = live(core, context)?.root;
    for segment in path {
        match segment {
            Seg::Key(key) => {
                if live(core, context)?.nodes[current].base.is_array() {
                    // String members of arrays resolve through
                    // `Array.prototype` upstream; the draft surface cannot
                    // address them.
                    *failure = Some(WalkFailure::Undefined);
                    return Ok(None);
                }
                if !object_has(core, context, current, key)? {
                    *failure = Some(WalkFailure::Undefined);
                    return Ok(None);
                }
                let value = read_object_position(core, context, current, key)?;
                let Some(value) = value else {
                    *failure = Some(WalkFailure::Undefined);
                    return Ok(None);
                };
                if value.is_null() {
                    *failure = Some(WalkFailure::Null);
                    return Ok(None);
                }
                if !value.is_object() && !value.is_array() {
                    *failure = Some(WalkFailure::Primitive(value));
                    return Ok(None);
                }
                current = child_for_key(core, context, current, key.clone())?;
            }
            Seg::Index(index) => {
                {
                    let overlay = live(core, context)?;
                    if !overlay.nodes[current].base.is_array() {
                        *failure = Some(WalkFailure::Undefined);
                        return Ok(None);
                    }
                    if *index >= array_length(overlay, current) {
                        *failure = Some(WalkFailure::Undefined);
                        return Ok(None);
                    }
                }
                let (source, source_index, from, placement) = {
                    let overlay = live(core, context)?;
                    let (source, source_index) = locate_piece(overlay, current, *index);
                    let reference = entry_ref(overlay, current, source, source_index);
                    let placement = match source {
                        None => matches!(reference, EntryRef::Slot { .. }),
                        Some(_) => true,
                    };
                    (source, source_index, reference.from(), placement)
                };
                current = child_for_entry(
                    core,
                    context,
                    current,
                    source,
                    source_index,
                    from,
                    placement,
                )?;
            }
        }
    }
    Ok(Some(current))
}

/// Reuse a cached child node for an object-key position when its `from`
/// still matches; otherwise create one (upstream `createNode` keyed by the
/// base container object).
fn child_for_key(
    core: &mut TrackerCore,
    context: usize,
    parent: usize,
    key: String,
) -> Result<usize, TrackerError> {
    let from = position_from(core, context, parent, &key)?;
    {
        let overlay = live(core, context)?;
        if let Some(&child) = overlay.nodes[parent].child_keys.get(&key) {
            if overlay.nodes[child].from == from {
                return Ok(child);
            }
        }
    }
    let base = read_object_position(core, context, parent, &key)?.unwrap_or(JsonValue::Null);
    let placement = find_write(&live(core, context)?.nodes[parent], &key).is_some();
    let child = {
        let overlay = live_mut(core, context)?;
        create_node(
            overlay,
            base,
            Some(parent),
            ParentKind::Object,
            ParentKey::Key(key.clone()),
            None,
            placement,
            from,
        )
    };
    live_mut(core, context)?.nodes[parent]
        .child_keys
        .insert(key, child);
    Ok(child)
}

fn child_for_entry(
    core: &mut TrackerCore,
    context: usize,
    parent: usize,
    source: Option<u64>,
    source_index: usize,
    from: Origin,
    placement: bool,
) -> Result<usize, TrackerError> {
    let cache_key = (source.is_none(), source.unwrap_or(0), source_index);
    {
        let overlay = live(core, context)?;
        if let Some(&child) = overlay.nodes[parent].child_entries.get(&cache_key) {
            if overlay.nodes[child].from == from {
                return Ok(child);
            }
        }
    }
    let base = read_entry_position(core, context, parent, source, source_index)?
        .unwrap_or(JsonValue::Null);
    let (parent_kind, parent_key, parent_source) = match source {
        None => (ParentKind::BaseEntry, ParentKey::Source(source_index), None),
        Some(source) => (
            ParentKind::InsertEntry,
            ParentKey::Source(source_index),
            Some(source),
        ),
    };
    let child = {
        let overlay = live_mut(core, context)?;
        create_node(
            overlay,
            base,
            Some(parent),
            parent_kind,
            parent_key,
            parent_source,
            placement,
            from,
        )
    };
    live_mut(core, context)?.nodes[parent]
        .child_entries
        .insert(cache_key, child);
    Ok(child)
}

// ─── Object overlay records ──────────────────────────────────────────────────

fn find_write(node: &OverlayNode, key: &str) -> Option<WriteValue> {
    if node.write_key.as_deref() == Some(key) {
        return node.write_value.clone();
    }
    node.writes
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.clone())
}

fn is_object_deleted(node: &OverlayNode, key: &str) -> bool {
    node.delete_key.as_deref() == Some(key) || node.deletes.iter().any(|at| at == key)
}

/// `objectHas` (`tracker.ts:749-752`).
fn object_has(
    core: &TrackerCore,
    context: usize,
    node: usize,
    key: &str,
) -> Result<bool, TrackerError> {
    let overlay = live(core, context)?;
    let node_ref = &overlay.nodes[node];
    if is_object_deleted(node_ref, key) {
        return Ok(false);
    }
    Ok(find_write(node_ref, key).is_some()
        || node_ref
            .base
            .as_object()
            .is_some_and(|object| object.contains_key(key)))
}

/// What the position currently points to as a [`From`].
fn position_from(
    core: &TrackerCore,
    context: usize,
    node: usize,
    key: &str,
) -> Result<Origin, TrackerError> {
    let overlay = live(core, context)?;
    if let Some(write) = find_write(&overlay.nodes[node], key) {
        return Ok(write.from());
    }
    Ok(Origin::Base)
}

/// Read the current overlay value at an object position; `None` is
/// upstream `undefined` (absent or deleted).
fn read_object_position(
    core: &TrackerCore,
    context: usize,
    node: usize,
    key: &str,
) -> Result<Option<JsonValue>, TrackerError> {
    let overlay = live(core, context)?;
    let node_ref = &overlay.nodes[node];
    if is_object_deleted(node_ref, key) {
        return Ok(None);
    }
    if let Some(write) = find_write(node_ref, key) {
        return Ok(Some(match write {
            WriteValue::Primitive(value) => value,
            WriteValue::Slot { slot, .. } => overlay.slot_value(slot),
        }));
    }
    Ok(node_ref
        .base
        .as_object()
        .and_then(|object| object.get(key))
        .cloned())
}

/// Read the current overlay value at an array entry.
fn read_entry_position(
    core: &TrackerCore,
    context: usize,
    node: usize,
    source: Option<u64>,
    source_index: usize,
) -> Result<Option<JsonValue>, TrackerError> {
    let overlay = live(core, context)?;
    Ok(match entry_ref(overlay, node, source, source_index) {
        EntryRef::Base => overlay.nodes[node]
            .base
            .as_array()
            .and_then(|array| array.get(source_index))
            .cloned(),
        EntryRef::Primitive(value) => Some(value),
        EntryRef::Slot { slot, .. } => Some(overlay.slot_value(slot)),
    })
}

/// `entryValueAt` (`tracker.ts:986-997`).
fn entry_ref(overlay: &Overlay, node: usize, source: Option<u64>, source_index: usize) -> EntryRef {
    if source.is_none() {
        if let Some((_, reference)) = overlay.nodes[node]
            .array
            .base_overrides
            .iter()
            .find(|(at, _)| *at == source_index)
        {
            return reference.clone();
        }
        return match overlay.nodes[node]
            .base
            .as_array()
            .and_then(|array| array.get(source_index))
        {
            Some(value) if value.is_object() || value.is_array() => EntryRef::Base,
            Some(value) => EntryRef::Primitive(value.clone()),
            None => EntryRef::Primitive(JsonValue::Null),
        };
    }
    let source = source.expect("checked above");
    if let Some((_, overrides)) = overlay.nodes[node]
        .array
        .insert_overrides
        .iter()
        .find(|(at, _)| *at == source)
    {
        if let Some((_, reference)) = overrides.iter().find(|(at, _)| *at == source_index) {
            return reference.clone();
        }
    }
    match overlay.sources[source as usize]
        .as_ref()
        .and_then(|refs| refs.get(source_index))
    {
        Some(EntryRef::Slot { slot, epoch }) => EntryRef::Slot {
            slot: *slot,
            epoch: *epoch,
        },
        Some(EntryRef::Primitive(value)) => EntryRef::Primitive(value.clone()),
        _ => EntryRef::Primitive(JsonValue::Null),
    }
}

/// `setObjectWrite` (`tracker.ts:689-711`) with
/// `storeValue`/`replaceStoredValue` slot reuse.
fn set_object_write(overlay: &mut Overlay, node: usize, key: String, value: JsonValue) {
    let existing_slot = find_write(&overlay.nodes[node], &key).and_then(|write| match write {
        WriteValue::Slot { slot, .. } => Some(slot),
        WriteValue::Primitive(_) => None,
    });
    let reference = overlay.overwrite(existing_slot, value);
    let write = match reference {
        EntryRef::Primitive(value) => WriteValue::Primitive(value),
        EntryRef::Slot { slot, epoch } => WriteValue::Slot { slot, epoch },
        EntryRef::Base => unreachable!("overwrite never returns Base"),
    };
    let node_ref = &mut overlay.nodes[node];
    if !node_ref.writes.is_empty() {
        match node_ref.writes.iter().position(|(name, _)| name == &key) {
            Some(at) => node_ref.writes[at].1 = write,
            None => node_ref.writes.push((key, write)),
        }
        return;
    }
    match &node_ref.write_key {
        Some(existing) if existing != &key => {
            let previous_key = node_ref.write_key.take().expect("checked above");
            let previous_value = node_ref.write_value.take().expect("checked above");
            node_ref.writes.push((previous_key, previous_value));
            node_ref.writes.push((key, write));
        }
        _ => {
            node_ref.write_key = Some(key);
            node_ref.write_value = Some(write);
        }
    }
}

/// `deleteObjectWrite` (`tracker.ts:713-723`).
fn delete_object_write(overlay: &mut Overlay, node: usize, key: &str) {
    if overlay.nodes[node].write_key.as_deref() == Some(key) {
        if let Some(WriteValue::Slot { slot, .. }) = overlay.nodes[node].write_value.take() {
            overlay.release_slot(slot);
        }
        overlay.nodes[node].write_key = None;
        return;
    }
    if let Some(at) = overlay.nodes[node]
        .writes
        .iter()
        .position(|(name, _)| name == key)
    {
        if let WriteValue::Slot { slot, .. } = overlay.nodes[node].writes.remove(at).1 {
            overlay.release_slot(slot);
        }
    }
}

/// `setObjectDeletion` (`tracker.ts:729-742`).
fn set_object_deletion(node: &mut OverlayNode, key: String) {
    if !node.deletes.is_empty() {
        if !node.deletes.contains(&key) {
            node.deletes.push(key);
        }
        return;
    }
    match &node.delete_key {
        Some(existing) if existing != &key => {
            let previous = node.delete_key.take().expect("checked above");
            node.deletes = vec![previous, key];
        }
        _ => node.delete_key = Some(key),
    }
}

/// `deleteObjectDeletion` (`tracker.ts:744-747`).
fn delete_object_deletion(node: &mut OverlayNode, key: &str) {
    if node.delete_key.as_deref() == Some(key) {
        node.delete_key = None;
    } else {
        node.deletes.retain(|at| at != key);
    }
}

/// The `set` trap over objects and arrays (`setProperty`,
/// `tracker.ts:563-591`).
fn set_property(
    core: &mut TrackerCore,
    context: usize,
    node: usize,
    key: Seg,
    value: JsonValue,
) -> Result<(), TrackerError> {
    if is_array_base(core, context, node) {
        return match key {
            Seg::Key(_) => Err(type_error("Only array indices and length can be written")),
            Seg::Index(index) => {
                let length = array_length_at(core, context, node);
                if index > length {
                    return Err(type_error("Overlay arrays cannot contain holes"));
                }
                set_array_index(core, context, node, index, value);
                Ok(())
            }
        };
    }
    let Seg::Key(key) = key else {
        return Err(type_error("Symbol writes are not supported"));
    };
    let was_deleted = {
        let overlay = live(core, context)?;
        is_object_deleted(&overlay.nodes[node], &key)
    };
    if !was_deleted {
        // `!isContainer(stored) && current === stored` for primitives.
        let current = {
            let overlay = live(core, context)?;
            let node_ref = &overlay.nodes[node];
            match find_write(node_ref, &key) {
                Some(WriteValue::Primitive(value)) => Some(value),
                Some(WriteValue::Slot { .. }) => None,
                None => node_ref
                    .base
                    .as_object()
                    .and_then(|object| object.get(&key))
                    .cloned(),
            }
        };
        if !value.is_object() && !value.is_array() && current.as_ref() == Some(&value) {
            return Ok(());
        }
    }
    let overlay = live_mut(core, context)?;
    set_object_write(overlay, node, key.clone(), value);
    let overlay = live_mut(core, context)?;
    if was_deleted
        && overlay.nodes[node]
            .base
            .as_object()
            .is_some_and(|object| object.contains_key(&key))
        && !overlay.nodes[node].readded.contains(&key)
    {
        overlay.nodes[node].readded.push(key.clone());
    }
    delete_object_deletion(&mut overlay.nodes[node], &key);
    mark_dirty(overlay, node);
    Ok(())
}

/// The `deleteProperty` trap over objects (`tracker.ts:593-604`).
fn delete_property(
    core: &mut TrackerCore,
    context: usize,
    node: usize,
    key: String,
) -> Result<(), TrackerError> {
    if !object_has(core, context, node, &key)? {
        return Ok(());
    }
    let overlay = live_mut(core, context)?;
    delete_object_write(overlay, node, &key);
    overlay.nodes[node].readded.retain(|at| at != &key);
    set_object_deletion(&mut overlay.nodes[node], key);
    mark_dirty(overlay, node);
    Ok(())
}

/// `markDirty` (`tracker.ts:761-766`).
fn mark_dirty(overlay: &mut Overlay, node: usize) {
    if overlay.nodes[node].dirty {
        return;
    }
    overlay.nodes[node].dirty = true;
    overlay.dirty.push(node);
    let mut current = overlay.nodes[node].parent;
    while let Some(parent) = current {
        overlay.nodes[parent].subtree_dirty = true;
        current = overlay.nodes[parent].parent;
    }
}

// ─── Array overlay (piece table) ─────────────────────────────────────────────

fn array_length(overlay: &Overlay, node: usize) -> usize {
    overlay.nodes[node]
        .array
        .pieces
        .iter()
        .map(Piece::length)
        .sum()
}

fn array_length_at(core: &TrackerCore, context: usize, node: usize) -> usize {
    array_length(live(core, context).expect("live overlay"), node)
}

/// `locatePiece` (`tracker.ts:955-969`): the piece covering `index`,
/// resolved to (insert source, source index); `None` source is a base
/// entry.
fn locate_piece(overlay: &Overlay, node: usize, index: usize) -> (Option<u64>, usize) {
    let mut remaining = index;
    for piece in &overlay.nodes[node].array.pieces {
        let length = piece.length();
        if remaining < length {
            let source_index = (piece.start() as i64 + piece.step() * remaining as i64) as usize;
            return match piece {
                Piece::Base { .. } => (None, source_index),
                Piece::Insert { source, .. } => (Some(*source), source_index),
            };
        }
        remaining -= length;
    }
    panic!("Array overlay index is out of range")
}

/// `getArrayIndex` value resolution (`tracker.ts:545-561`).
fn get_array_index_value(
    core: &TrackerCore,
    context: usize,
    node: usize,
    index: usize,
) -> Option<JsonValue> {
    let overlay = live(core, context).expect("live overlay");
    if index >= array_length(overlay, node) {
        return None;
    }
    let (source, source_index) = locate_piece(overlay, node, index);
    Some(match entry_ref(overlay, node, source, source_index) {
        EntryRef::Base => overlay.nodes[node]
            .base
            .as_array()
            .and_then(|array| array.get(source_index))
            .cloned()
            .unwrap_or(JsonValue::Null),
        EntryRef::Primitive(value) => value,
        EntryRef::Slot { slot, .. } => overlay.slot_value(slot),
    })
}

/// `insertPiece` (`tracker.ts:1068-1073`): one fresh insert source holding
/// the stored items.
fn insert_pieces(
    core: &mut TrackerCore,
    context: usize,
    node: usize,
    items: Vec<JsonValue>,
) -> Vec<Piece> {
    if items.is_empty() {
        return Vec::new();
    }
    let overlay = live_mut(core, context).expect("live overlay");
    let mut refs = Vec::with_capacity(items.len());
    for item in items {
        refs.push(overlay.store(item));
    }
    let source = overlay.next_source;
    overlay.next_source += 1;
    let length = refs.len();
    overlay.sources.push(Some(refs));
    let _ = node;
    vec![Piece::Insert {
        source,
        start: 0,
        length,
        step: 1,
    }]
}

/// The splice mutator's `insertPlacementPiece(node, args, 2)`
/// (`tracker.ts:1209-1226`): the two leading argument slots remain
/// primitive junk refs and the piece starts at offset 2 inside its source.
fn splice_pieces(
    core: &mut TrackerCore,
    context: usize,
    start: usize,
    remove: usize,
    items: Vec<JsonValue>,
) -> Vec<Piece> {
    if items.is_empty() {
        return Vec::new();
    }
    let overlay = live_mut(core, context).expect("live overlay");
    let mut refs: Vec<EntryRef> = vec![
        EntryRef::Primitive(super::number_json(start)),
        EntryRef::Primitive(super::number_json(remove)),
    ];
    for item in items {
        refs.push(overlay.store(item));
    }
    let source = overlay.next_source;
    overlay.next_source += 1;
    let length = refs.len() - 2;
    overlay.sources.push(Some(refs));
    vec![Piece::Insert {
        source,
        start: 2,
        length,
        step: 1,
    }]
}

/// `setArrayIndex` (`tracker.ts:1096-1132`).
fn set_array_index(
    core: &mut TrackerCore,
    context: usize,
    node: usize,
    index: usize,
    value: JsonValue,
) {
    let length = array_length_at(core, context, node);
    if index == length {
        let pieces = insert_pieces(core, context, node, vec![value]);
        replace_piece_range(core, context, node, length, 0, pieces);
        return;
    }
    {
        // `!isContainer(stored) && current === stored` early return.
        let overlay = live(core, context).expect("live overlay");
        let (source, source_index) = locate_piece(overlay, node, index);
        let current = match entry_ref(overlay, node, source, source_index) {
            EntryRef::Base => overlay.nodes[node]
                .base
                .as_array()
                .and_then(|array| array.get(source_index))
                .cloned(),
            EntryRef::Primitive(value) => Some(value),
            EntryRef::Slot { slot, .. } => Some(overlay.slot_value(slot)),
        };
        if !value.is_object() && !value.is_array() && current.as_ref() == Some(&value) {
            return;
        }
    }
    let (source, source_index, base_equal) = {
        let overlay = live(core, context).expect("live overlay");
        let (source, source_index) = locate_piece(overlay, node, index);
        let base_equal = match source {
            None => {
                !value.is_object()
                    && !value.is_array()
                    && overlay.nodes[node]
                        .base
                        .as_array()
                        .and_then(|array| array.get(source_index))
                        .is_some_and(|at| at == &value)
            }
            _ => false,
        };
        (source, source_index, base_equal)
    };
    let overlay = live_mut(core, context).expect("live overlay");
    match source {
        None => {
            let existing = overlay.nodes[node]
                .array
                .base_overrides
                .iter()
                .position(|(at, _)| *at == source_index);
            let existing_slot =
                existing.and_then(|at| match &overlay.nodes[node].array.base_overrides[at].1 {
                    EntryRef::Slot { slot, .. } => Some(*slot),
                    _ => None,
                });
            if base_equal {
                if let Some(at) = existing {
                    let (_, reference) = overlay.nodes[node].array.base_overrides.remove(at);
                    if let EntryRef::Slot { slot, .. } = reference {
                        overlay.release_slot(slot);
                    }
                }
            } else {
                let reference = overlay.overwrite(existing_slot, value);
                match existing {
                    None => overlay.nodes[node]
                        .array
                        .base_overrides
                        .push((source_index, reference)),
                    Some(at) => overlay.nodes[node].array.base_overrides[at].1 = reference,
                }
            }
        }
        Some(source_id) => {
            let position = overlay.nodes[node]
                .array
                .insert_overrides
                .iter()
                .position(|(at, _)| *at == source_id);
            let existing_entry = position.and_then(|at| {
                overlay.nodes[node].array.insert_overrides[at]
                    .1
                    .iter()
                    .position(|(entry, _)| *entry == source_index)
                    .map(|entry_at| (at, entry_at))
            });
            let existing_slot = existing_entry.and_then(|(at, entry_at)| {
                match &overlay.nodes[node].array.insert_overrides[at].1[entry_at].1 {
                    EntryRef::Slot { slot, .. } => Some(*slot),
                    _ => None,
                }
            });
            let refs_equal = existing_entry.is_none()
                && match overlay.sources[source_id as usize]
                    .as_ref()
                    .and_then(|refs| refs.get(source_index))
                {
                    Some(EntryRef::Primitive(prev)) => prev == &value,
                    _ => false,
                };
            if refs_equal {
                if let Some((at, entry_at)) = existing_entry {
                    let (_, reference) = overlay.nodes[node].array.insert_overrides[at]
                        .1
                        .remove(entry_at);
                    if let EntryRef::Slot { slot, .. } = reference {
                        overlay.release_slot(slot);
                    }
                    if overlay.nodes[node].array.insert_overrides[at].1.is_empty() {
                        overlay.nodes[node].array.insert_overrides.remove(at);
                    }
                }
            } else {
                let reference = overlay.overwrite(existing_slot, value);
                let at = match position {
                    Some(at) => at,
                    None => {
                        overlay.nodes[node]
                            .array
                            .insert_overrides
                            .push((source_id, Vec::new()));
                        overlay.nodes[node].array.insert_overrides.len() - 1
                    }
                };
                match overlay.nodes[node].array.insert_overrides[at]
                    .1
                    .iter()
                    .position(|(entry, _)| *entry == source_index)
                {
                    None => overlay.nodes[node].array.insert_overrides[at]
                        .1
                        .push((source_index, reference)),
                    Some(entry_at) => {
                        overlay.nodes[node].array.insert_overrides[at].1[entry_at].1 = reference;
                    }
                }
            }
        }
    }
    mark_dirty(overlay, node);
}

/// `setArrayLength` (`tracker.ts:1134-1148`).
fn set_array_length(core: &mut TrackerCore, context: usize, node: usize, next: usize) {
    let current = array_length_at(core, context, node);
    if next == current {
        return;
    }
    if next < current {
        replace_piece_range(core, context, node, next, current - next, Vec::new());
    } else {
        let items: Vec<JsonValue> = (0..next - current).map(|_| JsonValue::Null).collect();
        let pieces = insert_pieces(core, context, node, items);
        replace_piece_range(core, context, node, current, 0, pieces);
    }
}

/// `clampIndex` (`tracker.ts:1162-1166`).
fn clamp_index(value: isize, length: usize) -> usize {
    if value < 0 {
        (length as isize + value).max(0) as usize
    } else {
        (value as usize).min(length)
    }
}

/// `replacePieceRange` (`tracker.ts:1018-1049`) over the flat piece list.
fn replace_piece_range(
    core: &mut TrackerCore,
    context: usize,
    node: usize,
    index: usize,
    remove: usize,
    inserted: Vec<Piece>,
) {
    if remove == 0 && inserted.is_empty() {
        return;
    }
    let length = array_length_at(core, context, node);
    // Fast path: appending to an insert piece whose source still ends at
    // the piece tail (`tracker.ts:1021-1039`).
    if remove == 0 && index == length && inserted.len() == 1 {
        let addition = match &inserted[0] {
            Piece::Insert {
                source,
                start,
                length,
                ..
            } => Some((*source, *start, *length)),
            _ => None,
        };
        let tail = live(core, context).ok().and_then(|overlay| {
            overlay.nodes[node]
                .array
                .pieces
                .last()
                .and_then(|tail| match tail {
                    Piece::Insert {
                        source,
                        start,
                        length,
                        step,
                    } if *step == 1 => Some((*source, *start, *length)),
                    _ => None,
                })
        });
        if let (
            Some((addition_source, addition_start, addition_length)),
            Some((tail_source, tail_start, tail_length)),
        ) = (addition, tail)
        {
            let refs_len = live(core, context)
                .ok()
                .and_then(|overlay| overlay.sources[tail_source as usize].as_ref().map(Vec::len))
                .unwrap_or(0);
            if tail_start + tail_length == refs_len {
                let addition_refs: Vec<EntryRef> = {
                    let overlay = live(core, context).expect("live overlay");
                    overlay.sources[addition_source as usize]
                        .as_ref()
                        .expect("live source")[addition_start..addition_start + addition_length]
                        .to_vec()
                };
                let overlay = live_mut(core, context).expect("live overlay");
                let tail_refs = overlay.sources[tail_source as usize]
                    .as_mut()
                    .expect("live source");
                tail_refs.extend(addition_refs);
                overlay.sources[addition_source as usize] = None;
                if let Some(tail_piece) = overlay.nodes[node].array.pieces.last_mut() {
                    let new_length = tail_piece.length() + addition_length;
                    tail_piece.set_length(new_length);
                }
                overlay.nodes[node].array.structural = true;
                overlay.nodes[node].array.generation += 1;
                overlay.nodes[node].array.plan = None;
                mark_dirty(overlay, node);
                return;
            }
        }
    }
    let overlay = live_mut(core, context).expect("live overlay");
    let pieces = std::mem::take(&mut overlay.nodes[node].array.pieces);
    let (left, rest) = split_pieces(pieces, index);
    let (_removed, right) = split_pieces(rest, remove);
    let mut merged = left;
    merge_pieces(&mut merged);
    let mut middle = inserted;
    merge_pieces(&mut middle);
    merged.extend(middle);
    merged.extend(right);
    merge_pieces(&mut merged);
    overlay.nodes[node].array.pieces = merged;
    overlay.nodes[node].array.structural = true;
    overlay.nodes[node].array.generation += 1;
    overlay.nodes[node].array.plan = None;
    mark_dirty(overlay, node);
}

/// `splitPieceTree` (`tracker.ts:834-877`) over the flat list: split at a
/// logical index, cutting an interior piece in two when needed.
fn split_pieces(mut pieces: Vec<Piece>, index: usize) -> (Vec<Piece>, Vec<Piece>) {
    let mut offset = 0usize;
    for at in 0..pieces.len() {
        let length = pieces[at].length();
        if index < offset + length {
            if index == offset {
                let right = pieces.split_off(at);
                return (pieces, right);
            }
            let inside = index - offset;
            let original = pieces[at].clone();
            let mut left_piece = original.clone();
            left_piece.set_length(inside);
            let mut right_piece = original;
            let shifted =
                (right_piece.start() as i64 + right_piece.step() * inside as i64) as usize;
            right_piece.set_start(shifted);
            right_piece.set_length(length - inside);
            // Replace the original piece with its two halves.
            let mut tail = pieces.split_off(at);
            tail.remove(0);
            let mut left = pieces;
            left.push(left_piece);
            let mut right = vec![right_piece];
            right.extend(tail);
            return (left, right);
        }
        offset += length;
    }
    (pieces, Vec::new())
}

/// `mergePieces` (`tracker.ts:1051-1066`).
fn merge_pieces(pieces: &mut Vec<Piece>) {
    let mut index = 1usize;
    while index < pieces.len() {
        let merge = {
            let (left, right) = (&pieces[index - 1], &pieces[index]);
            let same_source = match (left, right) {
                (Piece::Base { .. }, Piece::Base { .. }) => true,
                (Piece::Insert { source: a, .. }, Piece::Insert { source: b, .. }) => a == b,
                _ => false,
            };
            if !same_source {
                false
            } else if left.length() == 1 && right.length() == 1 {
                (right.start() as i64 - left.start() as i64).abs() == 1
            } else {
                left.step() == right.step()
                    && left.start() as i64 + left.step() * left.length() as i64
                        == right.start() as i64
            }
        };
        if merge {
            if pieces[index - 1].length() == 1 && pieces[index].length() == 1 {
                let step = pieces[index].start() as i64 - pieces[index - 1].start() as i64;
                pieces[index - 1].set_step(step);
                pieces[index - 1].set_length(2);
            } else {
                let new_length = pieces[index - 1].length() + pieces[index].length();
                pieces[index - 1].set_length(new_length);
            }
            pieces.remove(index);
        } else {
            index += 1;
        }
    }
}

/// `reverse` (`tracker.ts:1227-1243`).
fn reverse_array(core: &mut TrackerCore, context: usize, node: usize) {
    if array_length_at(core, context, node) < 2 {
        return;
    }
    let overlay = live_mut(core, context).expect("live overlay");
    let mut pieces = std::mem::take(&mut overlay.nodes[node].array.pieces);
    pieces.reverse();
    for piece in &mut pieces {
        let new_start =
            (piece.start() as i64 + piece.step() * (piece.length() as i64 - 1)) as usize;
        piece.set_start(new_start);
        piece.set_step(if piece.step() == 1 { -1 } else { 1 });
    }
    overlay.nodes[node].array.pieces = pieces;
    overlay.nodes[node].array.structural = true;
    overlay.nodes[node].array.generation += 1;
    overlay.nodes[node].array.plan = None;
    mark_dirty(overlay, node);
}

/// `sort` (`tracker.ts:1244-1313`).
fn sort_array(
    core: &mut TrackerCore,
    context: usize,
    node: usize,
    comparator: &mut dyn FnMut(&JsonValue, &JsonValue) -> i32,
) {
    // Walk the pieces collecting the token order plus resolved comparator
    // values (`publicSortValue`, `tracker.ts:1347-1376`).
    let (mut order, values, sources, indices) = {
        let mut inserted_values: Vec<JsonValue> = Vec::new();
        let mut inserted_sources: Vec<u64> = Vec::new();
        let mut inserted_indices: Vec<usize> = Vec::new();
        let mut order: Vec<i64> = Vec::new();
        let pieces: Vec<Piece> = live(core, context).expect("live overlay").nodes[node]
            .array
            .pieces
            .clone();
        for piece in &pieces {
            for offset in 0..piece.length() {
                let source_index = (piece.start() as i64 + piece.step() * offset as i64) as usize;
                match piece {
                    Piece::Base { .. } => order.push(source_index as i64),
                    Piece::Insert { source, .. } => {
                        let token = -(inserted_sources.len() as i64) - 1;
                        order.push(token);
                        inserted_sources.push(*source);
                        inserted_indices.push(source_index);
                        inserted_values.push(
                            read_entry_position(core, context, node, Some(*source), source_index)
                                .ok()
                                .flatten()
                                .unwrap_or(JsonValue::Null),
                        );
                    }
                }
            }
        }
        (order, inserted_values, inserted_sources, inserted_indices)
    };
    let base_snapshot: Vec<(usize, JsonValue)> = {
        let overlay = live(core, context).expect("live overlay");
        overlay.nodes[node]
            .array
            .base_overrides
            .iter()
            .map(|(index, reference)| (*index, entry_ref_value(overlay, reference)))
            .collect()
    };
    let insert_snapshots: Vec<(u64, Vec<(usize, JsonValue)>)> = {
        let overlay = live(core, context).expect("live overlay");
        overlay.nodes[node]
            .array
            .insert_overrides
            .iter()
            .map(|(source, overrides)| {
                (
                    *source,
                    overrides
                        .iter()
                        .map(|(index, reference)| (*index, entry_ref_value(overlay, reference)))
                        .collect(),
                )
            })
            .collect()
    };
    let has_overrides = !base_snapshot.is_empty() || !insert_snapshots.is_empty();
    {
        let values = &values;
        order.sort_by(|left, right| {
            let left_value = if *left < 0 {
                values[(-left - 1) as usize].clone()
            } else {
                read_entry_position(core, context, node, None, *left as usize)
                    .ok()
                    .flatten()
                    .unwrap_or(JsonValue::Null)
            };
            let right_value = if *right < 0 {
                values[(-right - 1) as usize].clone()
            } else {
                read_entry_position(core, context, node, None, *right as usize)
                    .ok()
                    .flatten()
                    .unwrap_or(JsonValue::Null)
            };
            comparator(&left_value, &right_value).cmp(&0)
        });
    }
    if has_overrides {
        restore_sort_overrides(
            core,
            context,
            node,
            &order,
            &sources,
            &indices,
            &base_snapshot,
            &insert_snapshots,
        );
    }
    let current_length = array_length_at(core, context, node);
    let same_prefix = {
        let overlay = live(core, context).expect("live overlay");
        current_length >= order.len()
            && order.iter().enumerate().all(|(logical, token)| {
                same_sort_token_at(overlay, node, logical, *token, &sources, &indices)
            })
    };
    if !same_prefix {
        let pieces = pieces_from_sort_order(&order, &sources, &indices);
        replace_piece_range(
            core,
            context,
            node,
            0,
            order.len().min(current_length),
            pieces,
        );
    }
}

fn entry_ref_value(overlay: &Overlay, reference: &EntryRef) -> JsonValue {
    match reference {
        EntryRef::Base => JsonValue::Null,
        EntryRef::Primitive(value) => value.clone(),
        EntryRef::Slot { slot, .. } => overlay.slot_value(*slot),
    }
}

/// `restoreSortOverride` loop (`tracker.ts:1288-1297, 1378-1417`).
#[allow(clippy::too_many_arguments)]
fn restore_sort_overrides(
    core: &mut TrackerCore,
    context: usize,
    node: usize,
    order: &[i64],
    sources: &[u64],
    indices: &[usize],
    base_snapshot: &[(usize, JsonValue)],
    insert_snapshots: &[(u64, Vec<(usize, JsonValue)>)],
) {
    for &token in order {
        if token >= 0 {
            let source_index = token as usize;
            let snapshot = base_snapshot
                .iter()
                .find(|(index, _)| *index == source_index)
                .map(|(_, value)| value.clone());
            let overlay = live_mut(core, context).expect("live overlay");
            let existing = overlay.nodes[node]
                .array
                .base_overrides
                .iter()
                .position(|(index, _)| *index == source_index);
            match snapshot {
                Some(value) => {
                    let existing_slot = existing.and_then(|at| {
                        match &overlay.nodes[node].array.base_overrides[at].1 {
                            EntryRef::Slot { slot, .. } => Some(*slot),
                            _ => None,
                        }
                    });
                    let reference = overlay.overwrite(existing_slot, value);
                    match existing {
                        None => overlay.nodes[node]
                            .array
                            .base_overrides
                            .push((source_index, reference)),
                        Some(at) => overlay.nodes[node].array.base_overrides[at].1 = reference,
                    }
                }
                None => {
                    if let Some(at) = existing {
                        let (_, reference) = overlay.nodes[node].array.base_overrides.remove(at);
                        if let EntryRef::Slot { slot, .. } = reference {
                            overlay.release_slot(slot);
                        }
                    }
                }
            }
            continue;
        }
        let at = (-token - 1) as usize;
        let source = sources[at];
        let source_index = indices[at];
        let snapshot = insert_snapshots
            .iter()
            .find(|(at, _)| *at == source)
            .and_then(|(_, overrides)| {
                overrides
                    .iter()
                    .find(|(index, _)| *index == source_index)
                    .map(|(_, value)| value.clone())
            });
        let overlay = live_mut(core, context).expect("live overlay");
        let position = overlay.nodes[node]
            .array
            .insert_overrides
            .iter()
            .position(|(at, _)| *at == source);
        match snapshot {
            Some(value) => {
                let existing_slot = position.and_then(|at| {
                    overlay.nodes[node].array.insert_overrides[at]
                        .1
                        .iter()
                        .find(|(entry, _)| *entry == source_index)
                        .and_then(|(_, reference)| match reference {
                            EntryRef::Slot { slot, .. } => Some(*slot),
                            _ => None,
                        })
                });
                let reference = overlay.overwrite(existing_slot, value);
                let position = match position {
                    Some(position) => position,
                    None => {
                        overlay.nodes[node]
                            .array
                            .insert_overrides
                            .push((source, Vec::new()));
                        overlay.nodes[node].array.insert_overrides.len() - 1
                    }
                };
                match overlay.nodes[node].array.insert_overrides[position]
                    .1
                    .iter()
                    .position(|(entry, _)| *entry == source_index)
                {
                    None => overlay.nodes[node].array.insert_overrides[position]
                        .1
                        .push((source_index, reference)),
                    Some(entry_at) => {
                        overlay.nodes[node].array.insert_overrides[position].1[entry_at].1 =
                            reference;
                    }
                }
            }
            None => {
                if let Some(at) = position {
                    if let Some(entry_at) = overlay.nodes[node].array.insert_overrides[at]
                        .1
                        .iter()
                        .position(|(entry, _)| *entry == source_index)
                    {
                        let (_, reference) = overlay.nodes[node].array.insert_overrides[at]
                            .1
                            .remove(entry_at);
                        if let EntryRef::Slot { slot, .. } = reference {
                            overlay.release_slot(slot);
                        }
                    }
                    if overlay.nodes[node].array.insert_overrides[at].1.is_empty() {
                        overlay.nodes[node].array.insert_overrides.remove(at);
                    }
                }
            }
        }
    }
}

/// `sameSortTokenAt` (`tracker.ts:1419-1434`).
fn same_sort_token_at(
    overlay: &Overlay,
    node: usize,
    logical: usize,
    token: i64,
    sources: &[u64],
    indices: &[usize],
) -> bool {
    if logical >= array_length(overlay, node) {
        return false;
    }
    let (source, source_index) = locate_piece(overlay, node, logical);
    match (source, token < 0) {
        (None, false) => source_index == token as usize,
        (Some(at), true) => {
            source_index == indices[(-token - 1) as usize] && at == sources[(-token - 1) as usize]
        }
        _ => false,
    }
}

/// `piecesFromSortOrder` (`tracker.ts:1475-1509`): reuses the original
/// insert sources so run merging matches upstream.
fn pieces_from_sort_order(order: &[i64], sources: &[u64], indices: &[usize]) -> Vec<Piece> {
    let mut pieces: Vec<Piece> = Vec::new();
    for &token in order {
        if token < 0 {
            let at = (-token - 1) as usize;
            append_merged_piece(
                &mut pieces,
                Piece::Insert {
                    source: sources[at],
                    start: indices[at],
                    length: 1,
                    step: 1,
                },
            );
        } else {
            append_merged_piece(
                &mut pieces,
                Piece::Base {
                    start: token as usize,
                    length: 1,
                    step: 1,
                },
            );
        }
    }
    pieces
}

/// `appendMergedPiece` (`tracker.ts:1511-1533`).
fn append_merged_piece(pieces: &mut Vec<Piece>, piece: Piece) {
    let matches_last = pieces
        .last()
        .map(|previous| match (previous, &piece) {
            (Piece::Base { .. }, Piece::Base { .. }) => true,
            (Piece::Insert { source: a, .. }, Piece::Insert { source: b, .. }) => a == b,
            _ => false,
        })
        .unwrap_or(false);
    if let Some(previous) = pieces.last_mut() {
        let same_source = matches_last;
        if same_source && previous.length() == 1 && piece.length() == 1 {
            let step = piece.start() as i64 - previous.start() as i64;
            if step == 1 || step == -1 {
                previous.set_step(step);
                previous.set_length(2);
                return;
            }
        }
        if same_source
            && previous.step() == piece.step()
            && previous.start() as i64 + previous.step() * previous.length() as i64
                == piece.start() as i64
        {
            let new_length = previous.length() + piece.length();
            previous.set_length(new_length);
            return;
        }
    }
    pieces.push(piece);
}

// ─── Path resolution for emission ────────────────────────────────────────────

/// `resolvePath` (`tracker.ts:2036-2058`): walk parents; a node's emission
/// path only resolves while every parent position still holds the container
/// the node was created for. (Upstream caches successes in
/// `preparedPath`; the result is deterministic, so the port recomputes.)
fn resolve_path(core: &TrackerCore, context: usize, node: usize) -> Option<Path> {
    let parent = match live(core, context).ok()?.nodes.get(node)?.parent {
        Some(parent) => parent,
        // The tracked root resolves to the empty path (`tracker.ts:2039-2042`).
        None => return Some(Path::new()),
    };
    let parent_path = resolve_path(core, context, parent)?;
    let overlay = live(core, context).ok()?;
    let node_ref = &overlay.nodes[node];
    let segment: Seg = match node_ref.parent_kind {
        ParentKind::Object => {
            let ParentKey::Key(key) = &node_ref.parent_key else {
                return None;
            };
            // `objectHas(parent, key) && objectValue(parent, key) ===
            // nodeBase(node)`.
            let from = if is_object_deleted(&overlay.nodes[parent], key) {
                Origin::Scalar
            } else {
                match find_write(&overlay.nodes[parent], key) {
                    Some(write) => write.from(),
                    None => Origin::Base,
                }
            };
            if from != node_ref.from {
                return None;
            }
            if from == Origin::Base
                && !overlay.nodes[parent]
                    .base
                    .as_object()
                    .is_some_and(|object| object.contains_key(key))
            {
                return None;
            }
            Seg::Key(key.clone())
        }
        ParentKind::BaseEntry | ParentKind::InsertEntry => {
            let ParentKey::Source(source_index) = &node_ref.parent_key else {
                return None;
            };
            let index = find_entry_index(
                overlay,
                parent,
                node_ref.parent_kind,
                *source_index,
                node_ref.parent_source,
            )?;
            let (source, at) = locate_piece(overlay, parent, index);
            if entry_ref(overlay, parent, source, at).from() != node_ref.from {
                return None;
            }
            Seg::Index(index)
        }
    };
    let mut path = parent_path;
    path.push(segment);
    Some(path)
}

/// `findEntryIndex` (`tracker.ts:2089-2110`).
fn find_entry_index(
    overlay: &Overlay,
    node: usize,
    kind: ParentKind,
    source_index: usize,
    source: Option<u64>,
) -> Option<usize> {
    let array = &overlay.nodes[node].array;
    if kind == ParentKind::BaseEntry && !array.structural {
        return Some(source_index);
    }
    // `ensurePieceLocations` (`tracker.ts:2060-2087`): locations ordered by
    // minimum source index.
    let mut locations: Vec<(usize, usize, usize, usize, i64)> = Vec::new();
    let mut logical_start = 0usize;
    for (at, piece) in array.pieces.iter().enumerate() {
        let last = piece.start() as i64 + piece.step() * (piece.length() as i64 - 1);
        let minimum = piece.start().min(last.max(0) as usize);
        let matches = match (kind, piece) {
            (ParentKind::BaseEntry, Piece::Base { .. }) => true,
            (ParentKind::InsertEntry, Piece::Insert { source: at, .. }) => Some(*at) == source,
            _ => false,
        };
        if matches {
            locations.push((
                minimum,
                logical_start,
                piece.start(),
                piece.length(),
                at as i64,
            ));
        }
        logical_start += piece.length();
    }
    locations.sort_by_key(|(minimum, _, _, _, _)| *minimum);
    let mut low = 0usize;
    let mut high = locations.len();
    while low < high {
        let middle = (low + high) / 2;
        if locations[middle].0 <= source_index {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    let (_, logical_start, start, length, at) = *locations.get(low.checked_sub(1)?)?;
    let step = array.pieces[at as usize].step();
    let last = start as i64 + step * (length as i64 - 1);
    let maximum = start.max(last.max(0) as usize);
    if source_index > maximum || step == 0 {
        return None;
    }
    let offset = (source_index as i64 - start as i64) / step;
    if offset < 0 || offset >= length as i64 {
        return None;
    }
    Some(logical_start + offset as usize)
}

// ─── Materialization and op emission ─────────────────────────────────────────

/// `materializePrepared` (`tracker.ts:325-341`) plus `ensureOperations`
/// (`tracker.ts:2112-2122`): returns `(base, value, ops)`.
fn prepare_context(
    core: &mut TrackerCore,
    context: usize,
) -> Result<(JsonValue, JsonValue, Vec<Op>), TrackerError> {
    let operations = ensure_operations(core, context)?;
    let (base, root, simple) = {
        let overlay = live(core, context)?;
        (
            overlay.base_value.clone(),
            overlay.root,
            overlay.simple_object_materialization,
        )
    };
    let value = if operations.is_empty() {
        base.clone()
    } else if simple {
        clone_node(core, context, root)
    } else {
        materialize_operations(&base, &operations)?
    };
    Ok((base, value, operations))
}

/// `materializeOperations` (`tracker.ts:343-349`).
fn materialize_operations(base: &JsonValue, operations: &[Op]) -> Result<JsonValue, TrackerError> {
    if operations
        .iter()
        .any(|operation| matches!(operation, Op::Splice { .. } | Op::Reorder { .. }))
    {
        return Ok(super::apply_immutable(Some(base), operations)?);
    }
    Ok(apply_immutable_trusted(base, operations)?)
}

/// `ensureOperations` (`tracker.ts:2112-2122`).
fn ensure_operations(core: &mut TrackerCore, context: usize) -> Result<Vec<Op>, TrackerError> {
    if let Some(ops) = live(core, context)?.ops.clone() {
        return Ok(ops);
    }
    assert_readable(core, context)?;
    let replacement = live(core, context)?.replacement;
    if replacement {
        let matches_revision = core.contexts[context].base_revision == core.revision;
        let current_value = core.value.clone();
        let overlay = live_mut(core, context)?;
        let root_base = overlay.nodes[overlay.root].base.clone();
        let noop = matches_revision && current_value == root_base;
        overlay.replacement_noop = noop;
        let ops = if noop {
            Vec::new()
        } else {
            vec![Op::Replace(root_base)]
        };
        overlay.ops = Some(ops);
    } else {
        let ops = emit_operations(core, context);
        live_mut(core, context)?.ops = Some(ops);
    }
    Ok(live(core, context)?.ops.clone().expect("just stored"))
}

/// `emitOperations` (`tracker.ts:1602-1674`).
fn emit_operations(core: &mut TrackerCore, context: usize) -> Vec<Op> {
    if let Some(simple) = emit_simple_object_operations(core, context) {
        return simple;
    }
    let mut operations: Vec<Op> = Vec::new();
    // (node, path) in insertion order; `positions` overrides paths in place
    // (a Map.set on an existing key keeps its insertion position).
    let mut emission: Vec<(usize, Path)> = Vec::new();
    let mut positions: HashMap<usize, usize> = HashMap::new();
    let mut forced: Vec<usize> = Vec::new();
    let mut forced_paths: HashMap<usize, Path> = HashMap::new();

    let dirty: Vec<usize> = live(core, context).expect("live overlay").dirty.clone();
    // Dense candidates over base-array overrides (`tracker.ts:1609-1631`).
    let mut dense_candidates: HashMap<usize, DenseCandidates> = HashMap::new();
    for node in &dirty {
        record_dense_array_position(core, context, *node, &mut dense_candidates);
        let overlay = live(core, context).expect("live overlay");
        if overlay.nodes[*node].base.is_array() {
            let array = &overlay.nodes[*node].array;
            if !array.structural && !array.base_overrides.is_empty() {
                let length = overlay.nodes[*node].base.as_array().map_or(0, Vec::len);
                let candidates = dense_candidates
                    .entry(*node)
                    .or_insert_with(|| DenseCandidates::new(length));
                for (index, _) in &array.base_overrides {
                    add_dense_candidate(candidates, *index);
                }
            }
        }
    }
    let mut dense_regions: HashMap<usize, Vec<DenseRegion>> = HashMap::new();
    let candidate_nodes: Vec<usize> = dense_candidates.keys().copied().collect();
    for node in candidate_nodes {
        let Some(path) = resolve_path(core, context, node) else {
            continue;
        };
        if path
            .iter()
            .any(|segment| matches!(segment, Seg::Key(key) if is_reserved(key)))
        {
            continue;
        }
        let regions = build_dense_regions(&dense_candidates[&node]);
        if !regions.is_empty() {
            dense_regions.insert(node, regions);
        }
    }
    for node in &dirty {
        let Some(path) = resolve_path(core, context, *node) else {
            continue;
        };
        if has_covering_dense_region(core, context, *node, &path, &dense_regions) {
            continue;
        }
        match positions.get(node) {
            Some(&at) => emission[at].1 = path.clone(),
            None => {
                positions.insert(*node, emission.len());
                emission.push((*node, path.clone()));
            }
        }
        {
            let overlay = live(core, context).expect("live overlay");
            if !overlay.nodes[*node].base.is_array()
                && has_reserved_mutation(&overlay.nodes[*node])
                && !forced.contains(node)
            {
                forced.push(*node);
                forced_paths.insert(*node, path.clone());
            }
        }
        if let Some(reserved_at) = path
            .iter()
            .position(|segment| matches!(segment, Seg::Key(key) if is_reserved(key)))
        {
            // Fold the ancestor sitting at the reserved depth.
            let overlay = live(core, context).expect("live overlay");
            let mut ancestor = *node;
            let mut depth = path.len();
            while depth > reserved_at {
                ancestor = overlay.nodes[ancestor]
                    .parent
                    .expect("reserved path implies ancestors");
                depth -= 1;
            }
            if !forced.contains(&ancestor) {
                forced.push(ancestor);
                forced_paths.insert(ancestor, path[..reserved_at].to_vec());
            }
        }
    }
    for (node, path) in &forced_paths {
        match positions.get(node) {
            Some(&at) => emission[at].1 = path.clone(),
            None => {
                positions.insert(*node, emission.len());
                emission.push((*node, path.clone()));
            }
        }
    }
    let region_nodes: Vec<usize> = dense_regions.keys().copied().collect();
    for node in region_nodes {
        if let Some(path) = resolve_path(core, context, node) {
            if !has_covering_dense_region(core, context, node, &path, &dense_regions)
                && !positions.contains_key(&node)
            {
                positions.insert(node, emission.len());
                emission.push((node, path));
            }
        }
    }

    let mut max_depth = 0usize;
    for (_, path) in &emission {
        max_depth = max_depth.max(path.len());
    }
    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); max_depth + 1];
    for (at, (_, path)) in emission.iter().enumerate() {
        buckets[path.len()].push(at);
    }
    let mut folded: Vec<usize> = Vec::new();
    for bucket in &buckets {
        for at in bucket {
            let (node, path) = (&emission[*at].0, &emission[*at].1);
            let node = *node;
            if has_placement_ancestor(core, context, node)
                || has_folded_ancestor(core, context, node, &folded)
            {
                continue;
            }
            if forced.contains(&node) {
                let path = forced_paths
                    .get(&node)
                    .cloned()
                    .unwrap_or_else(|| path.clone());
                let value = clone_node(core, context, node);
                emit_set_op(&mut operations, &path, value);
                folded.push(node);
                continue;
            }
            let is_array = live(core, context).expect("live overlay").nodes[node]
                .base
                .is_array();
            if is_array {
                emit_array_operations(
                    core,
                    context,
                    node,
                    path,
                    &mut operations,
                    dense_regions.get(&node),
                );
            } else {
                emit_object_operations(core, context, node, path, &mut operations);
            }
            if operations.len() > MAX_DELTA_OPERATIONS {
                let root = live(core, context).expect("live overlay").root;
                let value = clone_node(core, context, root);
                return vec![Op::Replace(value)];
            }
        }
    }
    operations
}

struct DenseCandidates {
    indices: Vec<usize>,
    bits: Option<Vec<u8>>,
    length: usize,
}

impl DenseCandidates {
    fn new(length: usize) -> DenseCandidates {
        DenseCandidates {
            indices: Vec::new(),
            bits: None,
            length,
        }
    }
}

struct DenseRegion {
    start: usize,
    length: usize,
}

/// `addDenseCandidate` (`tracker.ts:1762-1772`).
fn add_dense_candidate(candidates: &mut DenseCandidates, index: usize) {
    if let Some(bits) = &mut candidates.bits {
        bits[index] = 1;
        return;
    }
    candidates.indices.push(index);
    if candidates.indices.len() < DENSE_CANDIDATE_BITS {
        return;
    }
    let mut bits = vec![0u8; candidates.length];
    for existing in &candidates.indices {
        bits[*existing] = 1;
    }
    candidates.indices.clear();
    candidates.bits = Some(bits);
}

/// `recordDenseArrayPosition`/`locateDenseArrayPosition`
/// (`tracker.ts:1716-1782`).
fn record_dense_array_position(
    core: &TrackerCore,
    context: usize,
    node: usize,
    groups: &mut HashMap<usize, DenseCandidates>,
) {
    let overlay = match live(core, context) {
        Ok(overlay) => overlay,
        Err(_) => return,
    };
    let mut child = node;
    loop {
        let parent = match overlay.nodes.get(child).and_then(|at| at.parent) {
            Some(parent) => parent,
            None => return,
        };
        if overlay.nodes[parent].base.is_array() {
            let array = &overlay.nodes[parent].array;
            if overlay.nodes[child].parent_kind != ParentKind::BaseEntry || array.structural {
                return;
            }
            let ParentKey::Source(source_index) = overlay.nodes[child].parent_key.clone() else {
                return;
            };
            // `current !== nodeBase(child)`: the entry must still resolve to
            // the container the child was created for.
            let reference = entry_ref(overlay, parent, None, source_index);
            if reference.from() != overlay.nodes[child].from {
                return;
            }
            let candidates = groups.entry(parent).or_insert_with(|| {
                DenseCandidates::new(overlay.nodes[parent].base.as_array().map_or(0, Vec::len))
            });
            add_dense_candidate(candidates, source_index);
            return;
        }
        child = parent;
    }
}

/// `buildDenseRegions` (`tracker.ts:1784-1807`).
fn build_dense_regions(candidates: &DenseCandidates) -> Vec<DenseRegion> {
    let Some(bits) = &candidates.bits else {
        return Vec::new();
    };
    let mut regions = Vec::new();
    let mut at = 0usize;
    while at < bits.len() {
        while at < bits.len() && bits[at] == 0 {
            at += 1;
        }
        if at == bits.len() {
            break;
        }
        let start = at;
        let mut end = at;
        let mut count = 0usize;
        let mut gap = 0usize;
        while at < bits.len() {
            if bits[at] != 0 {
                count += 1;
                end = at;
                gap = 0;
            } else {
                gap += 1;
                if gap > 1 {
                    break;
                }
            }
            at += 1;
        }
        let length = end - start + 1;
        if count >= DENSE_REGION_MIN_COUNT && count * 2 >= length {
            regions.push(DenseRegion { start, length });
        }
    }
    regions
}

fn region_containing(regions: &[DenseRegion], index: usize) -> bool {
    regions
        .iter()
        .any(|region| index >= region.start && index < region.start + region.length)
}

/// `hasCoveringDenseRegion` (`tracker.ts:1738-1760`).
fn has_covering_dense_region(
    core: &TrackerCore,
    context: usize,
    node: usize,
    path: &[Seg],
    dense_regions: &HashMap<usize, Vec<DenseRegion>>,
) -> bool {
    let overlay = match live(core, context) {
        Ok(overlay) => overlay,
        Err(_) => return false,
    };
    let mut current = node;
    while let Some(parent) = overlay.nodes[current].parent {
        current = parent;
        let Some(regions) = dense_regions.get(&current) else {
            continue;
        };
        let Some(parent_path) = resolve_path(core, context, current) else {
            continue;
        };
        if path.len() <= parent_path.len() {
            continue;
        }
        if path[..parent_path.len()] != parent_path[..] {
            continue;
        }
        if let Seg::Index(index) = path[parent_path.len()] {
            if region_containing(regions, index) {
                return true;
            }
        }
    }
    false
}

/// `hasReservedMutation` (`tracker.ts:1814-1824`).
fn has_reserved_mutation(node: &OverlayNode) -> bool {
    if node.write_key.as_deref().is_some_and(is_reserved) {
        return true;
    }
    if node.delete_key.as_deref().is_some_and(is_reserved) {
        return true;
    }
    if node.writes.iter().any(|(key, _)| is_reserved(key)) {
        return true;
    }
    node.deletes.iter().any(|key| is_reserved(key))
}

/// `hasFoldedAncestor` (`tracker.ts:1826-1830`).
fn has_folded_ancestor(core: &TrackerCore, context: usize, node: usize, folded: &[usize]) -> bool {
    let overlay = match live(core, context) {
        Ok(overlay) => overlay,
        Err(_) => return false,
    };
    let mut current = overlay.nodes[node].parent;
    while let Some(parent) = current {
        if folded.contains(&parent) {
            return true;
        }
        current = overlay.nodes[parent].parent;
    }
    false
}

/// `hasPlacementAncestor` (`tracker.ts:1832-1837`).
fn has_placement_ancestor(core: &TrackerCore, context: usize, node: usize) -> bool {
    let overlay = match live(core, context) {
        Ok(overlay) => overlay,
        Err(_) => return false,
    };
    let mut current = Some(node);
    while let Some(at) = current {
        if overlay.nodes[at].parent.is_some() && overlay.nodes[at].parent_placement {
            return true;
        }
        current = overlay.nodes[at].parent;
    }
    false
}

/// `emitSimpleObjectOperations` (`tracker.ts:1676-1711`).
fn emit_simple_object_operations(core: &mut TrackerCore, context: usize) -> Option<Vec<Op>> {
    let dirty: Vec<usize> = live(core, context).expect("live overlay").dirty.clone();
    let mut nodes: Vec<(usize, Path)> = Vec::new();
    for node in &dirty {
        {
            let overlay = live(core, context).expect("live overlay");
            if overlay.nodes[*node].base.is_array()
                || has_reserved_mutation(&overlay.nodes[*node])
                || has_placement_ancestor(core, context, *node)
            {
                return None;
            }
            let mut parent = overlay.nodes[*node].parent;
            while let Some(at) = parent {
                if overlay.nodes[at].base.is_array() {
                    return None;
                }
                parent = overlay.nodes[at].parent;
            }
        }
        let Some(path) = resolve_path(core, context, *node) else {
            continue;
        };
        nodes.push((*node, path));
        if nodes.len() > MAX_SIMPLE_OBJECT_NODES {
            return None;
        }
    }
    // Stable insertion sort by path length (`tracker.ts:1693-1702`).
    for index in 1..nodes.len() {
        let entry = nodes[index].clone();
        let depth = entry.1.len();
        let mut at = index;
        while at > 0 && nodes[at - 1].1.len() > depth {
            nodes[at] = nodes[at - 1].clone();
            at -= 1;
        }
        nodes[at] = entry;
    }
    let mut operations: Vec<Op> = Vec::new();
    let mut can_materialize_directly = true;
    for (node, path) in &nodes {
        if emit_object_operations(core, context, *node, path, &mut operations) {
            can_materialize_directly = false;
        }
        if operations.len() > MAX_DELTA_OPERATIONS {
            let root = live(core, context).expect("live overlay").root;
            let value = clone_node(core, context, root);
            return Some(vec![Op::Replace(value)]);
        }
    }
    live_mut(core, context)
        .expect("live overlay")
        .simple_object_materialization = can_materialize_directly;
    Some(operations)
}

/// `emitObjectOperations` (`tracker.ts:1850-1884`); returns
/// `normalizedContainerWrite`.
fn emit_object_operations(
    core: &mut TrackerCore,
    context: usize,
    node: usize,
    path: &[Seg],
    operations: &mut Vec<Op>,
) -> bool {
    let (readded, write_key, writes, delete_key, deletes) = {
        let overlay = live(core, context).expect("live overlay");
        let node_ref = &overlay.nodes[node];
        (
            node_ref.readded.clone(),
            node_ref.write_key.clone(),
            node_ref.writes.clone(),
            node_ref.delete_key.clone(),
            node_ref.deletes.clone(),
        )
    };
    for key in &readded {
        if base_has_key(core, context, node, key) {
            let mut next = path.to_owned();
            next.push(Seg::Key(key.clone()));
            operations.push(Op::Delete { path: next });
        }
    }
    let mut normalized_container_write = false;
    if let Some(key) = &write_key {
        let write = {
            let overlay = live(core, context).expect("live overlay");
            find_write(&overlay.nodes[node], key)
        };
        if let Some(write) = write {
            normalized_container_write |=
                emit_object_write(core, context, node, path, operations, key, &write);
        }
    }
    for (key, write) in &writes {
        if operations.len() > MAX_DELTA_OPERATIONS {
            return normalized_container_write;
        }
        normalized_container_write |=
            emit_object_write(core, context, node, path, operations, key, write);
    }
    if let Some(key) = &delete_key {
        if base_has_key(core, context, node, key) {
            let mut next = path.to_owned();
            next.push(Seg::Key(key.clone()));
            operations.push(Op::Delete { path: next });
        }
    }
    for key in &deletes {
        if operations.len() > MAX_DELTA_OPERATIONS {
            return normalized_container_write;
        }
        if base_has_key(core, context, node, key) {
            let mut next = path.to_owned();
            next.push(Seg::Key(key.clone()));
            operations.push(Op::Delete { path: next });
        }
    }
    normalized_container_write
}

fn base_has_key(core: &TrackerCore, context: usize, node: usize, key: &str) -> bool {
    live(core, context)
        .ok()
        .and_then(|overlay| overlay.nodes[node].base.as_object())
        .is_some_and(|object| object.contains_key(key))
}

/// `emitObjectWrite` (`tracker.ts:1839-1848`).
fn emit_object_write(
    core: &mut TrackerCore,
    context: usize,
    node: usize,
    path: &[Seg],
    operations: &mut Vec<Op>,
    key: &str,
    write: &WriteValue,
) -> bool {
    let mut next_path = path.to_owned();
    next_path.push(Seg::Key(key.to_owned()));
    let before = {
        let overlay = live(core, context).expect("live overlay");
        let node_ref = &overlay.nodes[node];
        if node_ref.readded.iter().any(|at| at == key)
            || !node_ref
                .base
                .as_object()
                .is_some_and(|object| object.contains_key(key))
        {
            None
        } else {
            node_ref
                .base
                .as_object()
                .and_then(|object| object.get(key))
                .cloned()
        }
    };
    let after = clone_write_value(core, context, node, key, write);
    let emitted = emit_changed_value(operations, &next_path, before.as_ref(), &after);
    before
        .as_ref()
        .is_some_and(|value| value.is_object() || value.is_array())
        && (after.is_object() || after.is_array())
        && !emitted
}

/// `emitArrayOperations` (`tracker.ts:1945-1992`).
fn emit_array_operations(
    core: &mut TrackerCore,
    context: usize,
    node: usize,
    path: &[Seg],
    operations: &mut Vec<Op>,
    dense_regions: Option<&Vec<DenseRegion>>,
) {
    if let Some(regions) = dense_regions {
        for region in regions {
            if operations.len() > MAX_DELTA_OPERATIONS {
                return;
            }
            let items = clone_array_region(core, context, node, region);
            operations.push(Op::Splice {
                path: path.to_owned(),
                index: region.start,
                remove: region.length,
                items,
            });
        }
    }
    let structural = live(core, context).expect("live overlay").nodes[node]
        .array
        .structural;
    if !structural {
        emit_array_base_overrides(core, context, node, path, operations, dense_regions);
        return;
    }
    let plan = build_array_plan(core, context, node);
    for at in (0..plan.remove_runs.len()).step_by(2) {
        if operations.len() > MAX_DELTA_OPERATIONS {
            return;
        }
        operations.push(Op::Splice {
            path: path.to_owned(),
            index: plan.remove_runs[at],
            remove: plan.remove_runs[at + 1],
            items: Vec::new(),
        });
    }
    if let Some(permutation) = &plan.permutation {
        operations.push(Op::Reorder {
            path: path.to_owned(),
            permutation: permutation.clone(),
        });
    }
    for run in (0..plan.insert_runs.len()).step_by(3) {
        if operations.len() > MAX_DELTA_OPERATIONS {
            return;
        }
        let logical_index = plan.insert_runs[run];
        let start_piece = plan.insert_runs[run + 1];
        let end_piece = plan.insert_runs[run + 2];
        let mut items: Vec<JsonValue> = Vec::new();
        {
            let overlay = live(core, context).expect("live overlay");
            for piece_index in start_piece..end_piece {
                let piece = &overlay.nodes[node].array.pieces[piece_index];
                for offset in 0..piece.length() {
                    let source_index =
                        (piece.start() as i64 + piece.step() * offset as i64) as usize;
                    let (source, at) = match piece {
                        Piece::Base { .. } => (None, source_index),
                        Piece::Insert { source, .. } => (Some(*source), source_index),
                    };
                    items.push(clone_entry_value(core, context, node, source, at));
                }
            }
        }
        operations.push(Op::Splice {
            path: path.to_owned(),
            index: logical_index,
            remove: 0,
            items,
        });
    }
    emit_array_base_overrides(core, context, node, path, operations, dense_regions);
}

/// The `baseOverrides` loop of `emitArrayOperations` (`tracker.ts:1981-1991`).
fn emit_array_base_overrides(
    core: &mut TrackerCore,
    context: usize,
    node: usize,
    path: &[Seg],
    operations: &mut Vec<Op>,
    dense_regions: Option<&Vec<DenseRegion>>,
) {
    let overrides: Vec<(usize, EntryRef)> = {
        let overlay = live(core, context).expect("live overlay");
        overlay.nodes[node].array.base_overrides.clone()
    };
    for (base_index, _) in overrides {
        if operations.len() > MAX_DELTA_OPERATIONS {
            return;
        }
        let overlay = live(core, context).expect("live overlay");
        let Some(index) = find_entry_index(overlay, node, ParentKind::BaseEntry, base_index, None)
        else {
            continue;
        };
        if dense_regions.is_some_and(|regions| region_containing(regions, index)) {
            continue;
        }
        let before = overlay.nodes[node]
            .base
            .as_array()
            .and_then(|array| array.get(base_index))
            .cloned();
        let after = clone_entry_value(core, context, node, None, base_index);
        let mut next_path = path.to_owned();
        next_path.push(Seg::Index(index));
        emit_changed_value(operations, &next_path, before.as_ref(), &after);
    }
}

/// `buildArrayPlan` (`tracker.ts:1886-1937`).
fn build_array_plan(core: &mut TrackerCore, context: usize, node: usize) -> ArrayPlan {
    if let Some(plan) = &live(core, context).expect("live overlay").nodes[node]
        .array
        .plan
    {
        return ArrayPlan {
            remove_runs: plan.remove_runs.clone(),
            permutation: plan.permutation.clone(),
            insert_runs: plan.insert_runs.clone(),
        };
    }
    let overlay = live(core, context).expect("live overlay");
    let node_ref = &overlay.nodes[node];
    let base_length = node_ref.base.as_array().map_or(0, Vec::len);
    let mut retained = vec![0u8; base_length];
    let mut target_base: Vec<usize> = Vec::new();
    for piece in &node_ref.array.pieces {
        if let Piece::Base { .. } = piece {
            for offset in 0..piece.length() {
                let index = (piece.start() as i64 + piece.step() * offset as i64) as usize;
                if index < base_length {
                    retained[index] = 1;
                }
                target_base.push(index);
            }
        }
    }
    let mut remove_runs: Vec<usize> = Vec::new();
    let mut end = base_length;
    while end > 0 {
        if retained[end - 1] != 0 {
            end -= 1;
            continue;
        }
        let mut start = end - 1;
        while start > 0 && retained[start - 1] == 0 {
            start -= 1;
        }
        remove_runs.push(start);
        remove_runs.push(end - start);
        end = start;
    }
    let retained_base: Vec<usize> = (0..base_length).filter(|at| retained[*at] != 0).collect();
    let permutation = if target_base != retained_base {
        let positions: HashMap<usize, usize> = retained_base
            .iter()
            .enumerate()
            .map(|(index, value)| (*value, index))
            .collect();
        Some(
            target_base
                .iter()
                .map(|value| positions.get(value).copied().unwrap_or(0))
                .collect(),
        )
    } else {
        None
    };
    let mut insert_runs: Vec<usize> = Vec::new();
    let mut logical_index = 0usize;
    let mut piece_index = 0usize;
    while piece_index < node_ref.array.pieces.len() {
        let piece = &node_ref.array.pieces[piece_index];
        if piece.is_base() {
            logical_index += piece.length();
            piece_index += 1;
            continue;
        }
        let start_piece = piece_index;
        let mut run_length = 0usize;
        while piece_index < node_ref.array.pieces.len()
            && !node_ref.array.pieces[piece_index].is_base()
        {
            logical_index += node_ref.array.pieces[piece_index].length();
            run_length += node_ref.array.pieces[piece_index].length();
            piece_index += 1;
        }
        insert_runs.push(logical_index - run_length);
        insert_runs.push(start_piece);
        insert_runs.push(piece_index);
    }
    let plan = ArrayPlan {
        remove_runs,
        permutation,
        insert_runs,
    };
    live_mut(core, context).expect("live overlay").nodes[node]
        .array
        .plan = Some(ArrayPlan {
        remove_runs: plan.remove_runs.clone(),
        permutation: plan.permutation.clone(),
        insert_runs: plan.insert_runs.clone(),
    });
    plan
}

/// `cloneArrayRegion` (`tracker.ts:1994-2003`).
fn clone_array_region(
    core: &TrackerCore,
    context: usize,
    node: usize,
    region: &DenseRegion,
) -> Vec<JsonValue> {
    let overlay = live(core, context).expect("live overlay");
    let mut result = Vec::with_capacity(region.length);
    for index in region.start..region.start + region.length {
        let (source, source_index) = locate_piece(overlay, node, index);
        result.push(match entry_ref(overlay, node, source, source_index) {
            EntryRef::Base => overlay.nodes[node]
                .base
                .as_array()
                .and_then(|array| array.get(source_index))
                .cloned()
                .unwrap_or(JsonValue::Null),
            EntryRef::Primitive(value) => value,
            EntryRef::Slot { slot, .. } => overlay.slot_value(slot),
        });
    }
    result
}

/// `emitChangedValue` (`tracker.ts:2005-2029`).
fn emit_changed_value(
    operations: &mut Vec<Op>,
    path: &[Seg],
    before: Option<&JsonValue>,
    after: &JsonValue,
) -> bool {
    let after_is_container = after.is_object() || after.is_array();
    if let Some(before) = before {
        let before_is_container = before.is_object() || before.is_array();
        if !after_is_container && !before_is_container && before == after {
            return false;
        }
        if before_is_container && after_is_container && before == after {
            // `equalTrustedJson` over alias-free trees.
            return false;
        }
    }
    if let (Some(JsonValue::String(before)), JsonValue::String(after)) = (before, after) {
        if after.len() > before.len() && after.starts_with(before.as_str()) {
            operations.push(Op::Append {
                path: path.to_owned(),
                text: after[before.len()..].to_owned(),
            });
            return true;
        }
        let shared = overlap(before, after, 65_536);
        if shared > 0 {
            operations.push(Op::Truncate {
                path: path.to_owned(),
                count: utf16_len(before) - shared,
            });
            if utf16_len(after) > shared {
                operations.push(Op::Append {
                    path: path.to_owned(),
                    text: slice_utf16_from(after, shared).to_owned(),
                });
            }
            return true;
        }
    }
    operations.push(Op::Set {
        path: path.to_owned(),
        value: after.clone(),
    });
    true
}

/// `emitSet` (`tracker.ts:2031-2034`).
fn emit_set_op(operations: &mut Vec<Op>, path: &[Seg], value: JsonValue) {
    if path.is_empty() {
        operations.push(Op::Replace(value));
    } else {
        operations.push(Op::Set {
            path: path.to_owned(),
            value,
        });
    }
}

// ─── Clone helpers (cloneNode / cloneStored) ─────────────────────────────────

/// `cloneNode` (`tracker.ts:1546-1563`): the node's full overlay view as a
/// plain value.
fn clone_node(core: &TrackerCore, context: usize, node: usize) -> JsonValue {
    let overlay = live(core, context).expect("live overlay");
    if overlay.nodes[node].base.is_array() {
        let mut result: Vec<JsonValue> = Vec::new();
        let pieces: Vec<Piece> = overlay.nodes[node].array.pieces.clone();
        for piece in &pieces {
            for offset in 0..piece.length() {
                let source_index = (piece.start() as i64 + piece.step() * offset as i64) as usize;
                let (source, at) = match piece {
                    Piece::Base { .. } => (None, source_index),
                    Piece::Insert { source, .. } => (Some(*source), source_index),
                };
                result.push(clone_entry_value(core, context, node, source, at));
            }
        }
        JsonValue::Array(result)
    } else {
        let mut object = serde_json::Map::new();
        for segment in own_keys(overlay, node) {
            let Seg::Key(key) = segment else { continue };
            let value = clone_object_position(core, context, node, &key);
            object.insert(key, value);
        }
        JsonValue::Object(object)
    }
}

/// `cloneStored` for an object-key base position: fold a dirty
/// base-derived child view when one was created for this key.
fn clone_object_position(core: &TrackerCore, context: usize, node: usize, key: &str) -> JsonValue {
    let overlay = live(core, context).expect("live overlay");
    let node_ref = &overlay.nodes[node];
    if is_object_deleted(node_ref, key) {
        return JsonValue::Null;
    }
    if let Some(write) = find_write(node_ref, key) {
        return clone_write_value(core, context, node, key, &write);
    }
    if let Some(&child) = node_ref.child_keys.get(key) {
        if overlay.nodes[child].from == Origin::Base
            && (overlay.nodes[child].dirty || overlay.nodes[child].subtree_dirty)
        {
            return clone_node(core, context, child);
        }
    }
    node_ref
        .base
        .as_object()
        .and_then(|object| object.get(key))
        .cloned()
        .unwrap_or(JsonValue::Null)
}

/// `cloneStored` for an object-key write: fold a dirty child view when one
/// was created for the stored container at this key.
fn clone_write_value(
    core: &TrackerCore,
    context: usize,
    node: usize,
    key: &str,
    write: &WriteValue,
) -> JsonValue {
    let overlay = live(core, context).expect("live overlay");
    if let WriteValue::Slot { slot, epoch } = write {
        if let Some(&child) = overlay.nodes[node].child_keys.get(key) {
            if overlay.nodes[child].from
                == (Origin::Slot {
                    slot: *slot,
                    epoch: *epoch,
                })
                && (overlay.nodes[child].dirty || overlay.nodes[child].subtree_dirty)
            {
                return clone_node(core, context, child);
            }
        }
        return overlay.slot_value(*slot);
    }
    match write {
        WriteValue::Primitive(value) => value.clone(),
        WriteValue::Slot { slot, .. } => overlay.slot_value(*slot),
    }
}

/// `cloneStored` for an array-entry reference: fold a dirty child view
/// created for the referenced container.
fn clone_entry_value(
    core: &TrackerCore,
    context: usize,
    node: usize,
    source: Option<u64>,
    source_index: usize,
) -> JsonValue {
    let overlay = live(core, context).expect("live overlay");
    let reference = entry_ref(overlay, node, source, source_index);
    let cache_key = (source.is_none(), source.unwrap_or(0), source_index);
    if let Some(&child) = overlay.nodes[node].child_entries.get(&cache_key) {
        if overlay.nodes[child].from == reference.from()
            && (overlay.nodes[child].dirty || overlay.nodes[child].subtree_dirty)
        {
            return clone_node(core, context, child);
        }
    }
    match reference {
        EntryRef::Base => overlay.nodes[node]
            .base
            .as_array()
            .and_then(|array| array.get(source_index))
            .cloned()
            .unwrap_or(JsonValue::Null),
        EntryRef::Primitive(value) => value,
        EntryRef::Slot { slot, .. } => overlay.slot_value(slot),
    }
}

/// `ownKeys` (`tracker.ts:617-662`): numeric keys first (sorted), then the
/// string keys, with writes added and deletes/readds removed.
fn own_keys(overlay: &Overlay, node: usize) -> Vec<Seg> {
    let node_ref = &overlay.nodes[node];
    if node_ref.base.is_array() {
        let length = node_ref.array.pieces.iter().map(Piece::length).sum();
        return (0..length).map(Seg::Index).collect();
    }
    let mut existing_keys_only = node_ref.delete_key.is_none()
        && node_ref.deletes.is_empty()
        && node_ref.readded.is_empty()
        && match &node_ref.write_key {
            Some(key) => node_ref
                .base
                .as_object()
                .is_some_and(|object| object.contains_key(key)),
            None => true,
        };
    if existing_keys_only && !node_ref.writes.is_empty() {
        for (key, _) in &node_ref.writes {
            if !node_ref
                .base
                .as_object()
                .is_some_and(|object| object.contains_key(key))
            {
                existing_keys_only = false;
                break;
            }
        }
    }
    let mut keys: Vec<String> = Vec::new();
    if existing_keys_only {
        if let Some(object) = node_ref.base.as_object() {
            keys.extend(object.keys().cloned());
        }
    } else {
        if let Some(object) = node_ref.base.as_object() {
            for key in object.keys() {
                if !is_object_deleted(node_ref, key) && !node_ref.readded.iter().any(|at| at == key)
                {
                    keys.push(key.clone());
                }
            }
        }
        let mut seen: Vec<String> = keys.clone();
        if let Some(key) = &node_ref.write_key {
            if !seen.contains(key) {
                keys.push(key.clone());
                seen.push(key.clone());
            }
        }
        for (key, _) in &node_ref.writes {
            if !seen.contains(key) {
                keys.push(key.clone());
                seen.push(key.clone());
            }
        }
    }
    let mut indices: Vec<usize> = Vec::new();
    let mut strings: Vec<String> = Vec::new();
    for key in keys {
        match parse_array_index(&key) {
            Some(index) => indices.push(index),
            None => strings.push(key),
        }
    }
    indices.sort_unstable();
    let mut out: Vec<Seg> = indices.into_iter().map(Seg::Index).collect();
    out.extend(strings.into_iter().map(Seg::Key));
    out
}

/// `arrayIndex` (`tracker.ts:971-984`): canonical array-index strings only.
fn parse_array_index(property: &str) -> Option<usize> {
    if property.is_empty() || property.len() > 10 {
        return None;
    }
    if property == "0" {
        return Some(0);
    }
    let first = property.as_bytes()[0];
    if !(b'1'..=b'9').contains(&first) {
        return None;
    }
    let mut index: usize = (first - b'0') as usize;
    for digit in property.as_bytes()[1..].iter() {
        if !digit.is_ascii_digit() {
            return None;
        }
        index = index * 10 + (digit - b'0') as usize;
        if index >= 4_294_967_295 {
            return None;
        }
    }
    Some(index)
}

// ─── Abort / clear / invalidate ──────────────────────────────────────────────

/// `abortContext` (`tracker.ts:2141-2146`).
fn abort_context(core: &mut TrackerCore, context: usize) {
    let status = core.contexts.get(context).map(|cell| cell.status);
    match status {
        Some(Status::Aborted) | Some(Status::Consumed) | Some(Status::Stale) | None => {}
        Some(_) => {
            core.contexts[context].status = Status::Aborted;
            clear_context(core, context);
        }
    }
}

/// `clearContext` (`tracker.ts:2162-2201`): release the overlay; statuses
/// keep driving the settled checks.
fn clear_context(core: &mut TrackerCore, context: usize) {
    if let Some(cell) = core.contexts.get_mut(context) {
        cell.overlay = None;
    }
}

/// `#invalidate` (`tracker.ts:301-313`): mark competing drafts stale and
/// drop their overlays in O(1).
fn invalidate(core: &mut TrackerCore, winner: usize) {
    for at in 0..core.contexts.len() {
        if at == winner {
            continue;
        }
        if matches!(core.contexts[at].status, Status::Open | Status::Prepared) {
            core.contexts[at].status = Status::Stale;
        }
        if core.contexts[at].status == Status::Stale {
            core.contexts[at].overlay = None;
        }
    }
}

/// JS `typeof value` over the JSON union.
fn js_typeof(value: &JsonValue) -> &'static str {
    match value {
        JsonValue::Null => "null",
        JsonValue::Bool(_) => "boolean",
        JsonValue::Number(_) => "number",
        JsonValue::String(_) => "string",
        JsonValue::Array(_) | JsonValue::Object(_) => "object",
    }
}

/// JS `String(value)` for default sort keys (`tracker.ts:1284-1286`).
fn js_value_to_string(value: &JsonValue) -> String {
    match value {
        JsonValue::Null => "null".to_owned(),
        JsonValue::Bool(true) => "true".to_owned(),
        JsonValue::Bool(false) => "false".to_owned(),
        JsonValue::Number(number) => {
            super::diff::js_number_to_string(number.as_f64().unwrap_or_default())
        }
        JsonValue::String(text) => text.clone(),
        JsonValue::Array(_) | JsonValue::Object(_) => "[object Object]".to_owned(),
    }
}

fn split_path(path: &[Seg]) -> Result<(&[Seg], Seg), TrackerError> {
    match path.split_last() {
        Some((last, prefix)) => Ok((prefix, last.clone())),
        None => Err(type_error("Cannot set properties of undefined")),
    }
}
