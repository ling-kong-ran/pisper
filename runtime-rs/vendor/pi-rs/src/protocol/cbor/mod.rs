//! CBOR codec. Port of `packages/protocol/src/cbor/` (`index.ts`,
//! `options.ts`, `encoder.ts`, `decoder.ts`).
//!
//! Upstream works on dynamic JS values; the port carries them with
//! [`CborValue`], an owned tree that adds CBOR byte strings to the ordered
//! JSON tree [`crate::protocol::json::JsonValue`]. Divergences D1-D4 are
//! documented in the parent module docs.

pub mod decoder;
pub mod encoder;
pub mod options;

pub use decoder::decode_cbor;
pub use encoder::encode_cbor;
pub use options::{
    resolve_options, CborError, CborFailure, CborOptions, RangeError, ResolvedCborOptions,
    DEFAULT_MAX_CBOR_BYTE_LENGTH, DEFAULT_MAX_CBOR_CONTAINER_LENGTH, DEFAULT_MAX_CBOR_DEPTH,
    MAX_SAFE_INTEGER, MAX_UINT32, UINT32_BASE,
};

use crate::protocol::json::{JsonValue, Number};

/// A decoded or to-be-encoded CBOR value: the protocol's strict RFC 8949
/// subset (null, booleans, safe integers, finite float64s, byte/text strings,
/// definite-length arrays and string-keyed maps).
///
/// Maps are insertion-ordered vectors, matching the JS object key order that
/// upstream's encoder emits and its decoder preserves.
#[derive(Debug, Clone, PartialEq)]
pub enum CborValue {
    Null,
    Bool(bool),
    /// Major type 0 (unsigned integer).
    Uint(u64),
    /// Major type 1 (negative integer).
    Int(i64),
    /// Major type 7 float64; integral non-negative-zero floats encode through
    /// the integer path, exactly like upstream JS numbers.
    Float(f64),
    Text(String),
    Bytes(Vec<u8>),
    Array(Vec<CborValue>),
    Map(Vec<(String, CborValue)>),
}

impl CborValue {
    /// First value for `key` in a map.
    pub fn get(&self, key: &str) -> Option<&CborValue> {
        match self {
            CborValue::Map(entries) => entries
                .iter()
                .find(|(entry_key, _)| entry_key == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }
}

/// Converts a decoded CBOR value into the strict-JSON tree used by the
/// protocol's opaque payloads. Returns `None` for the non-JSON CBOR values
/// (byte strings, non-finite floats), which is where upstream's `isJsonValue`
/// gate rejects them (divergence D5).
pub fn cbor_value_to_json(value: &CborValue) -> Option<JsonValue> {
    Some(match value {
        CborValue::Null => JsonValue::Null,
        CborValue::Bool(value) => JsonValue::Bool(*value),
        CborValue::Uint(value) => JsonValue::Number(Number::Uint(*value)),
        CborValue::Int(value) => JsonValue::Number(Number::Int(*value)),
        CborValue::Float(value) if value.is_finite() => JsonValue::Number(Number::Float(*value)),
        CborValue::Float(_) => return None,
        CborValue::Text(value) => JsonValue::String(value.clone()),
        CborValue::Bytes(_) => return None,
        CborValue::Array(items) => {
            let mut result = Vec::with_capacity(items.len());
            for item in items {
                result.push(cbor_value_to_json(item)?);
            }
            JsonValue::Array(result)
        }
        CborValue::Map(entries) => {
            let mut result = Vec::with_capacity(entries.len());
            for (key, value) in entries {
                result.push((key.clone(), cbor_value_to_json(value)?));
            }
            JsonValue::Object(result)
        }
    })
}

/// Converts a strict-JSON value into a CBOR value. Floats stay floats here —
/// the encoder applies upstream's number dispatch (integral floats take the
/// integer path), so wire bytes match upstream for any JSON input.
pub fn json_value_to_cbor(value: &JsonValue) -> CborValue {
    match value {
        JsonValue::Null => CborValue::Null,
        JsonValue::Bool(value) => CborValue::Bool(*value),
        JsonValue::Number(Number::Uint(value)) => CborValue::Uint(*value),
        JsonValue::Number(Number::Int(value)) => CborValue::Int(*value),
        JsonValue::Number(Number::Float(value)) => CborValue::Float(*value),
        JsonValue::String(value) => CborValue::Text(value.clone()),
        JsonValue::Array(items) => CborValue::Array(items.iter().map(json_value_to_cbor).collect()),
        JsonValue::Object(entries) => CborValue::Map(
            entries
                .iter()
                .map(|(key, value)| (key.clone(), json_value_to_cbor(value)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_conversion_rejects_non_json_cbor_values() {
        assert!(cbor_value_to_json(&CborValue::Bytes(vec![1])).is_none());
        assert!(cbor_value_to_json(&CborValue::Float(f64::NAN)).is_none());
        assert!(cbor_value_to_json(&CborValue::Map(vec![(
            "a".into(),
            CborValue::Bytes(vec![])
        )]))
        .is_none());
        assert!(cbor_value_to_json(&CborValue::Array(
            vec![CborValue::Uint(1), CborValue::Null,]
        ))
        .is_some());
    }

    #[test]
    fn json_conversion_preserves_order_and_values() {
        let json = JsonValue::Object(vec![
            ("z".to_string(), JsonValue::uint(1)),
            ("a".to_string(), JsonValue::Number(Number::Float(2.5))),
        ]);
        let cbor = json_value_to_cbor(&json);
        assert_eq!(
            cbor,
            CborValue::Map(vec![
                ("z".into(), CborValue::Uint(1)),
                ("a".into(), CborValue::Float(2.5)),
            ])
        );
        assert_eq!(cbor_value_to_json(&cbor), Some(json));
    }
}
