//! Port of `packages/chord/src/delta/apply-immutable-trusted.ts` (upstream
//! sha256 `861e7f8b7f410326cfb9658c272ee0d5e6235def40c3304237e41a5a49d3ec69`):
//! `applyImmutableTrusted`, the tracker's materialization path. Applies
//! self-produced trusted operations while copying each touched container
//! exactly once.
//!
//! # Ownership adaptation
//!
//! Upstream threads an `owned` WeakSet so the first operation copies the
//! containers along its path and later operations mutate the already-private
//! copies in place, leaving every untouched subtree *shared* with the base
//! revision. Over owned [`JsonValue`] trees the working root is private from
//! the first operation (the caller's value is cloned on entry), so the
//! WeakSet's "already copied" condition is true everywhere by construction
//! and the function degenerates to an in-place application on a private
//! clone — the same final value, with no aliasing to observe. The structural
//! checks on the way (`copyPath`/`read`/`permuteTrusted` guards) are
//! unreachable for self-produced operations by construction and surface as
//! the same `TypeError` messages [`super::DeltaError::InvalidOp`] carries.

use super::{apply, DeltaError, JsonValue, Op};

/// Apply self-produced trusted operations to a private copy of `target`.
/// Port of `applyImmutableTrusted`
/// (`apply-immutable-trusted.ts:15-61`); see the module docs for the
/// ownership adaptation.
pub fn apply_immutable_trusted(
    target: &JsonValue,
    operations: &[Op],
) -> Result<JsonValue, DeltaError> {
    apply(Some(target), operations)
}
