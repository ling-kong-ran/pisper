//! Port of `packages/chord/src/delta/revision-validator.ts` (upstream sha256
//! `d4d5c62b67271ecfe3029b0619a333b4d8a96a2bb6b8015fe83bffd0f7718c09`):
//! `JsonRevisionValidator`, the replica-side structural guard applied to
//! every immutable revision before it is published.
//!
//! Every upstream rejection is a JS-object-model property that an owned
//! [`JsonValue`] tree cannot carry:
//!
//! - cycles (`ancestors`) — an owned tree cannot reference itself;
//! - symbol properties / accessor or non-enumerable descriptors —
//!   `serde_json` objects are plain string-keyed data properties;
//! - non-plain prototypes (`assertPlainObject`) — objects are plain maps;
//! - sparse arrays or extra/`length` keys (`assertDenseArray`) — arrays are
//!   dense `Vec`s;
//! - `undefined` values or non-finite numbers (`assertPrimitive`) — the
//!   union has neither.
//!
//! The validator is therefore a shape-preserving pass-through here. It is
//! kept as a stateful type with the upstream name because the replica holds
//! one per subscription (`services/state.ts`) and revalidation-skipping
//! (`#validated`) would be observable if the value language ever grows
//! shared containers; see the tracker module docs for the aliasing note.

#[derive(Debug, Default, Clone)]
pub struct JsonRevisionValidator;

impl JsonRevisionValidator {
    /// `validate(value)` (`revision-validator.ts:11-13`): returns the value
    /// unchanged. Over owned JSON trees every upstream `TypeError` is
    /// unrepresentable, so this cannot fail.
    pub fn validate(
        &self,
        value: &crate::chord::types::JsonValue,
    ) -> crate::chord::types::JsonValue {
        value.clone()
    }
}
