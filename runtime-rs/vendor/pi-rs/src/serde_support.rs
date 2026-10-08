//! Shared wire-format helpers. Never collapse an explicit JSON null into an
//! omitted TypeScript optional JsonValue/unknown field.
use serde::Deserialize;

mod js_numbers;
pub(crate) use js_numbers::{js_number_string, to_json_string_with_js_numbers};

pub(crate) fn present_json<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<serde_json::Value>, D::Error> {
    serde_json::Value::deserialize(deserializer).map(Some)
}

/// A JavaScript array-index property name. `2^32 - 1` is NOT an array index;
/// leading zeros, signs, whitespace, exponents and non-ASCII digits are names.
pub(crate) fn js_array_index(key: &str) -> Option<u32> {
    if key.is_empty()
        || (key.len() > 1 && key.starts_with('0'))
        || !key.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    key.parse::<u32>().ok().filter(|index| *index != u32::MAX)
}

/// JS own string-key enumeration: numeric indices first, then other strings
/// in insertion order. Stable sorting is essential for the latter group.
pub(crate) fn order_js_object_entries<T>(entries: &mut [(String, T)]) {
    entries.sort_by_key(|(key, _)| js_array_index(key).unwrap_or(u32::MAX));
}

/// Normalize only object enumeration order for a JavaScript JSON boundary.
/// This does not implement JS number formatting, undefined, or non-JSON values.
pub(crate) fn order_json_object_keys(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                order_json_object_keys(item);
            }
        }
        serde_json::Value::Object(fields) => {
            for value in fields.values_mut() {
                order_json_object_keys(value);
            }
            if fields.keys().any(|key| js_array_index(key).is_some()) {
                let mut entries: Vec<_> = std::mem::take(fields).into_iter().collect();
                order_js_object_entries(&mut entries);
                fields.extend(entries);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod json_order_tests {
    use super::{js_array_index, order_json_object_keys};

    #[test]
    fn json_wire_property_key_classifier_matches_array_index_boundaries() {
        for (key, index) in [("0", 0), ("1", 1), ("4294967294", u32::MAX - 1)] {
            assert_eq!(js_array_index(key), Some(index));
        }
        for key in [
            "",
            "00",
            "01",
            "-0",
            "+0",
            "0.0",
            "1e0",
            " 1",
            "1 ",
            "１２",
            "4294967295",
            "4294967296",
            "9007199254740991",
            "18446744073709551615",
        ] {
            assert_eq!(js_array_index(key), None, "{key}");
        }
    }

    #[test]
    fn json_wire_property_order_normalization_is_recursive_and_idempotent() {
        let mut value: serde_json::Value =
            serde_json::from_str(r#"{"z":{"10":1,"a":2,"2":3},"01":0,"1":[{"b":4,"0":5}],"a":6}"#)
                .unwrap();
        order_json_object_keys(&mut value);
        let once = serde_json::to_string(&value).unwrap();
        assert_eq!(
            once,
            r#"{"1":[{"0":5,"b":4}],"z":{"2":3,"10":1,"a":2},"01":0,"a":6}"#
        );
        order_json_object_keys(&mut value);
        assert_eq!(serde_json::to_string(&value).unwrap(), once);
    }
}
