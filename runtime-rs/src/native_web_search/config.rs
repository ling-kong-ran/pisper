use super::SearchError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchConfig {
    pub provider: String,
    pub language: String,
    pub safe_search: u8,
    pub max_results: u8,
}

impl WebSearchConfig {
    pub fn try_normalize(input: &Value) -> Result<Self, SearchError> {
        if input.is_null() {
            return Err(SearchError::invalid(
                "Cannot read properties of null (reading 'language')",
            ));
        }
        for field in ["language", "safeSearch", "maxResults"] {
            if let Some(value) = input.get(field) {
                if field != "language" || truthy(value) {
                    validate_js_conversion(value)?;
                }
            }
        }
        Ok(Self::normalize(input))
    }

    pub fn normalize(input: &Value) -> Self {
        let language = input.get("language").filter(|value| truthy(value));
        let language = language.map(js_string).unwrap_or_else(|| "auto".into());
        let language = slice_utf16(language.trim_matches(js_whitespace), 40);
        Self {
            provider: "bing".into(),
            language: if language.is_empty() {
                "auto".into()
            } else {
                language
            },
            safe_search: bounded_integer(input.get("safeSearch"), 1, 0, 2) as u8,
            max_results: bounded_integer(input.get("maxResults"), 8, 1, 12) as u8,
        }
    }
}

pub fn normalize_config(input: &Value) -> Value {
    let config = WebSearchConfig::normalize(input);
    json!({"provider":config.provider,"language":config.language,"safeSearch":config.safe_search,"maxResults":config.max_results})
}

/// Configuration writers can validate before mutating canonical pisper.json.
/// The older infallible normalizer remains for already-validated/default data.
pub fn normalize_config_checked(input: &Value) -> Result<Value, SearchError> {
    let config = WebSearchConfig::try_normalize(input)?;
    Ok(
        json!({"provider":config.provider,"language":config.language,"safeSearch":config.safe_search,"maxResults":config.max_results}),
    )
}

pub(super) fn validate_js_conversion(value: &Value) -> Result<(), SearchError> {
    match value {
        // JSON cannot contain callable valueOf/toString properties. An own
        // toString value shadows Object.prototype.toString, so ToPrimitive
        // fails, rather than silently yielding "[object Object]" or NaN.
        Value::Object(object) if object.contains_key("toString") => Err(SearchError::invalid(
            "Cannot convert object to primitive value",
        )),
        Value::Array(items) => {
            for item in items {
                validate_js_conversion(item)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

pub(super) fn js_string_checked(value: &Value) -> Result<String, SearchError> {
    validate_js_conversion(value)?;
    Ok(js_string(value))
}

pub(super) fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

pub(super) fn js_string(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => js_number_string(value.as_f64().unwrap_or_default()),
        Value::String(value) => value.clone(),
        Value::Array(value) => value
            .iter()
            .map(|item| {
                if item.is_null() {
                    String::new()
                } else {
                    js_string(item)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}

pub(super) fn js_number_string(number: f64) -> String {
    if number.is_nan() {
        return "NaN".into();
    }
    if number.is_infinite() {
        return if number.is_sign_negative() {
            "-Infinity".into()
        } else {
            "Infinity".into()
        };
    }
    if number == 0.0 {
        "0".into()
    } else if number.abs() >= 1e21 || number.abs() < 1e-6 {
        let text = format!("{number:e}");
        let (mantissa, exponent) = text.split_once('e').expect("scientific number");
        let exponent = exponent.parse::<i32>().expect("numeric exponent");
        format!(
            "{mantissa}e{}{exponent}",
            if exponent >= 0 { "+" } else { "" }
        )
    } else {
        number.to_string()
    }
}

fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Null => Some(0.0),
        Value::Bool(value) => Some(if *value { 1.0 } else { 0.0 }),
        Value::Number(value) => value.as_f64(),
        Value::String(_) | Value::Array(_) => {
            let text = js_string(value?);
            let text = text.trim_matches(js_whitespace);
            if text.is_empty() {
                return Some(0.0);
            }
            for (prefix, radix) in [
                ("0x", 16),
                ("0X", 16),
                ("0b", 2),
                ("0B", 2),
                ("0o", 8),
                ("0O", 8),
            ] {
                if let Some(digits) = text.strip_prefix(prefix) {
                    if digits.is_empty() {
                        return None;
                    }
                    let mut number = 0.0_f64;
                    for digit in digits.chars() {
                        number = number * radix as f64 + digit.to_digit(radix)? as f64;
                    }
                    return Some(number);
                }
            }
            // Rust 会接受 inf/NaN；JS 仅接受 Infinity，但有限性检查随后会拒绝两者。
            text.parse::<f64>().ok()
        }
        Value::Object(_) => None,
    }
}

pub(super) fn bounded_integer(
    value: Option<&Value>,
    fallback: usize,
    min: usize,
    max: usize,
) -> usize {
    number(value)
        .filter(|number| number.is_finite())
        .map(|number| {
            // Adding 0.5 first loses the low bit just below a half (for
            // example 0.49999999999999994). JS Math.round still rounds down.
            let floor = number.floor();
            let rounded = if number - floor < 0.5 {
                floor
            } else {
                floor + 1.0
            };
            rounded.clamp(min as f64, max as f64) as usize
        })
        .unwrap_or(fallback)
}

pub(super) fn js_whitespace(value: char) -> bool {
    matches!(value, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

pub(super) fn slice_utf16(value: &str, limit: usize) -> String {
    // 与 JS .slice 的计量单位一致；截断代理项采用 U+FFFD，保持两种客户端的合法 UTF-8 契约。
    String::from_utf16_lossy(&value.encode_utf16().take(limit).collect::<Vec<_>>())
}
