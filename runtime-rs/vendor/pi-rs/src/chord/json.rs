//! Strict-JSON predicates and copying. Port of `packages/chord/src/json.ts`
//! (upstream sha256 `8b7d633aabcf694f3827355a9d5bc305efbcdb6c276f9420509eeccfa613edf8`):
//! `copyJson` and `isJsonValue`.
//!
//! In a JS host these functions guard against the extra values objects can
//! carry: `undefined` properties, non-finite numbers, typed arrays, class
//! instances, accessor descriptors, and reference cycles. Over owned
//! [`JsonValue`] (`serde_json::Value`) trees all of those are
//! unrepresentable by construction — numbers are always finite, arrays are
//! dense plain arrays, objects are plain maps with enumerable data
//! properties, and cycles cannot exist in an owned tree. Upstream also
//! removed the historical 512-depth bound in this delta, so nothing remains
//! to check: [`is_json_value`] accepts every owned value, and [`copy_json`]
//! is a deep copy.

use crate::chord::types::JsonValue;

/// `CopyJsonOptions` (`json.ts:8-11`): `omitUndefinedProperties` drops
/// `undefined` object properties; owned trees have no `undefined`, so the
/// option has no representable effect and is accepted for API parity.
#[derive(Clone, Copy, Debug, Default)]
pub struct CopyJsonOptions {
    /// Omit undefined object properties while preserving strict array
    /// semantics (unrepresentable over owned trees).
    pub omit_undefined_properties: bool,
}

/// `copyJson(value, options)` (`json.ts:14-16`): copy a value into an
/// alias-free strict-JSON tree owned by the caller. Over owned trees the
/// validation walk and the copy coincide with a deep clone; cycles and
/// non-finite numbers cannot reach this function.
pub fn copy_json(value: &JsonValue, _options: Option<CopyJsonOptions>) -> JsonValue {
    value.clone()
}

/// `isJsonValue(value)` (`json.ts:117-119`): with the depth bound removed
/// and every remaining check unrepresentable over owned JSON trees, this is
/// constant `true`. Kept for API parity with the upstream export.
pub fn is_json_value(_value: &JsonValue) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn owned_trees_are_always_strict_json() {
        assert!(is_json_value(&json!({ "nested": [1, true, null] })));
        let deep = {
            let mut value = json!(0);
            for _ in 0..600 {
                value = json!([value]);
            }
            value
        };
        // The historical 512-depth bound is gone upstream.
        assert!(is_json_value(&deep));
        assert!(is_json_value(&json!(null)));
        assert!(is_json_value(&json!([])));
        assert!(is_json_value(&json!({})));
    }

    #[test]
    fn copy_json_clones() {
        let value = json!({ "a": [1, 2], "b": { "c": "d" } });
        let copied = copy_json(&value, None);
        assert_eq!(copied, value);
        assert!(copied.is_object());
    }
}
