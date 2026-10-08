//! Ordered strict-JSON value tree. Port of the `JsonValue` shape and the
//! `isJsonValue` contract from `@earendil-works/chord`
//! (`packages/chord/src/json.ts`), which upstream protocol values rely on.
//!
//! Objects are insertion-ordered `Vec<(String, JsonValue)>` rather than
//! `serde_json::Map`. Stored field order survives representation conversions
//! with serde_json/preserve_order; JS integer-index enumeration still needs
//! a separate protocol audit (D6 in the parent module docs). Every value that
//! can be constructed satisfies the upstream `isJsonValue` predicate: finite
//! numbers, no cycles, no holes, only string keys (divergence D5).
//!
//! [`JsonValue::from_serde_json`] / [`JsonValue::to_serde_json`] convert to
//! and from `serde_json::Value` for interop with the rest of the crate.

use serde_json::{Number as SerdeNumber, Value as SerdeValue};

/// A JSON number, mirroring `serde_json::Number`'s three representations.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Number {
    /// Non-negative integer (serde_json `PosInt`).
    Uint(u64),
    /// Negative integer (serde_json `NegInt`).
    Int(i64),
    /// Floating-point value; construction paths never produce non-finite
    /// values (`serde_json` cannot represent them either).
    Float(f64),
}

impl Number {
    /// Upstream `Number.isInteger` view of the value.
    pub fn is_integral(&self) -> bool {
        match self {
            Number::Float(value) => value.is_finite() && value.fract() == 0.0,
            _ => true,
        }
    }

    /// The exact integer value when [`Number::is_integral`], widened so
    /// integral floats (e.g. `8.0`) compare equal to their integer form, like
    /// JS numbers do.
    pub fn as_integer(&self) -> Option<i128> {
        match self {
            Number::Uint(value) => Some(i128::from(*value)),
            Number::Int(value) => Some(i128::from(*value)),
            Number::Float(value) if value.is_finite() && value.fract() == 0.0 => {
                Some(*value as i128)
            }
            _ => None,
        }
    }

    pub fn as_f64(&self) -> f64 {
        match self {
            Number::Uint(value) => *value as f64,
            Number::Int(value) => *value as f64,
            Number::Float(value) => *value,
        }
    }
}

/// Strict JSON value with insertion-ordered objects. The port of the protocol
/// package's opaque payload values (`JsonValue` from `@earendil-works/chord`).
#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<JsonValue>),
    Object(Vec<(String, JsonValue)>),
}

impl JsonValue {
    /// Builds an object from key-value pairs, preserving the given order.
    pub fn object(entries: Vec<(String, JsonValue)>) -> JsonValue {
        JsonValue::Object(entries)
    }

    pub fn string(value: impl Into<String>) -> JsonValue {
        JsonValue::String(value.into())
    }

    pub fn uint(value: u64) -> JsonValue {
        JsonValue::Number(Number::Uint(value))
    }

    /// The object entries in wire/construction order.
    pub fn as_object(&self) -> Option<&[(String, JsonValue)]> {
        match self {
            JsonValue::Object(entries) => Some(entries),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[JsonValue]> {
        match self {
            JsonValue::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            JsonValue::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            JsonValue::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<Number> {
        match self {
            JsonValue::Number(value) => Some(*value),
            _ => None,
        }
    }

    /// First value for `key` in an object (objects never carry duplicate
    /// keys on any construction path that crosses the wire).
    pub fn get(&self, key: &str) -> Option<&JsonValue> {
        self.as_object()?
            .iter()
            .find(|(entry_key, _)| entry_key == key)
            .map(|(_, value)| value)
    }

    /// Converts from `serde_json::Value`. Object key order becomes the
    /// `serde_json::Map` insertion order (preserve_order is enabled).
    pub fn from_serde_json(value: &SerdeValue) -> JsonValue {
        match value {
            SerdeValue::Null => JsonValue::Null,
            SerdeValue::Bool(value) => JsonValue::Bool(*value),
            SerdeValue::Number(value) => JsonValue::Number(convert_number(value)),
            SerdeValue::String(value) => JsonValue::String(value.clone()),
            SerdeValue::Array(items) => {
                JsonValue::Array(items.iter().map(JsonValue::from_serde_json).collect())
            }
            SerdeValue::Object(entries) => JsonValue::Object(
                entries
                    .iter()
                    .map(|(key, value)| (key.clone(), JsonValue::from_serde_json(value)))
                    .collect(),
            ),
        }
    }

    /// Converts into `serde_json::Value`, retaining the stored object order
    /// through serde_json/preserve_order. Non-finite floats
    /// (unconstructible through public paths) map to `0`.
    pub fn to_serde_json(&self) -> SerdeValue {
        match self {
            JsonValue::Null => SerdeValue::Null,
            JsonValue::Bool(value) => SerdeValue::Bool(*value),
            JsonValue::Number(value) => SerdeValue::Number(match *value {
                Number::Uint(value) => SerdeNumber::from(value),
                Number::Int(value) => SerdeNumber::from(value),
                Number::Float(value) => {
                    SerdeNumber::from_f64(value).unwrap_or_else(|| SerdeNumber::from(0))
                }
            }),
            JsonValue::String(value) => SerdeValue::String(value.clone()),
            JsonValue::Array(items) => {
                SerdeValue::Array(items.iter().map(JsonValue::to_serde_json).collect())
            }
            JsonValue::Object(entries) => SerdeValue::Object(
                entries
                    .iter()
                    .map(|(key, value)| (key.clone(), value.to_serde_json()))
                    .collect(),
            ),
        }
    }
}

fn convert_number(value: &SerdeNumber) -> Number {
    if let Some(value) = value.as_u64() {
        return Number::Uint(value);
    }
    if let Some(value) = value.as_i64() {
        return Number::Int(value);
    }
    Number::Float(value.as_f64().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_object_key_order() {
        let value = JsonValue::object(vec![
            ("z".to_string(), JsonValue::uint(1)),
            ("a".to_string(), JsonValue::uint(2)),
        ]);
        let keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, _)| key.as_str())
            .collect();
        assert_eq!(keys, ["z", "a"]);
    }

    #[test]
    fn serde_roundtrip_preserves_values_and_order() {
        let value = JsonValue::object(vec![
            ("b".to_string(), JsonValue::uint(1)),
            ("a".to_string(), JsonValue::string("x")),
            ("f".to_string(), JsonValue::Number(Number::Float(1.5))),
            (
                "n".to_string(),
                JsonValue::Number(Number::Int(-9007199254740991)),
            ),
        ]);
        let serde_value = value.to_serde_json();
        let back = JsonValue::from_serde_json(&serde_value);
        assert_eq!(value.get("a"), back.get("a"));
        assert_eq!(value.get("f"), back.get("f"));
        assert_eq!(value.get("n"), back.get("n"));
        // preserve_order keeps the interop conversion lossless for object order.
        let back_keys: Vec<&str> = back
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, _)| key.as_str())
            .collect();
        assert_eq!(back_keys, ["b", "a", "f", "n"]);
        assert_eq!(value, back);
    }

    #[test]
    fn integral_floats_compare_like_js_numbers() {
        let float = Number::Float(8.0);
        assert!(float.is_integral());
        assert_eq!(float.as_integer(), Some(8));
        assert!(!Number::Float(1.5).is_integral());
        assert_eq!(Number::Float(1.5).as_integer(), None);
        assert_eq!(Number::Uint(8).as_integer(), Some(8));
        assert_eq!(Number::Int(-8).as_integer(), Some(-8));
    }
}
