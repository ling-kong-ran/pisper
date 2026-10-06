//! Port of `src/harness/json.ts`: leaf-by-leaf JSON assignment that keeps
//! Chord's recorded deltas minimal.
//!
//! Divergence (structural, disclosed): upstream `assignJson(target, key,
//! value)` mutates a `Draft` document value in place (a plain JS container
//! the tracker observes); the port's document drafts read and write at
//! explicit paths, so the walk mirrors the upstream recursion exactly but
//! resolves `current` with [`DocumentDraft::read`] and issues
//! [`DocumentDraft::set`] / [`DocumentDraft::delete`] /
//! [`DocumentDraft::push`] at the depth the upstream mutation would touch —
//! container reassignments stay whole sets, string leaves stay
//! tracker-diffed, and key deletion maps to `delete`.

use serde_json::Value;

use super::super::errors::PlainError;
use super::super::session::transaction::DocumentDraft;

type Seg = crate::chord::delta::Seg;

/// Assign `value` into the draft at `path`, leaf by leaf. Chord records a
/// container assignment as one full set and only emits an append when a
/// string leaf is reassigned, so writing the partial whole would store and
/// publish the complete message on every flush (`json.ts` `assignJson`, the
/// recursion over `slots[key]`).
pub fn assign_json(target: &DocumentDraft, path: &[Seg], value: &Value) -> Result<(), PlainError> {
    let error = |error: crate::chord::delta::TrackerError| PlainError::new(error.message());
    let current = target.read(path).map_err(error)?.unwrap_or(Value::Null);
    match (&current, value) {
        (Value::Object(current), Value::Object(entries)) => {
            for name in current.keys() {
                if !entries.contains_key(name) {
                    let mut child = path.to_vec();
                    child.push(Seg::Key(name.clone()));
                    target.delete(&child).map_err(error)?;
                }
            }
            for (name, child) in entries {
                let mut child_path = path.to_vec();
                child_path.push(Seg::Key(name.clone()));
                assign_json(target, &child_path, child)?;
            }
            Ok(())
        }
        (Value::Array(current), Value::Array(items)) if current.len() <= items.len() => {
            for (index, item) in items.iter().enumerate() {
                let mut child_path = path.to_vec();
                child_path.push(Seg::Index(index));
                assign_json(target, &child_path, item)?;
            }
            Ok(())
        }
        _ => {
            if current == *value {
                return Ok(());
            }
            target.set(path, value.clone()).map_err(error)
        }
    }
}
