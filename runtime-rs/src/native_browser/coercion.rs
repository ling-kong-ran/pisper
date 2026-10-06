//! 浏览器服务的 JSON 可表达输入沿用 release 的 String/Number/UTF-16 边界。
use serde_json::Value;

pub(super) fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}
pub(super) fn whitespace(value: char) -> bool {
    matches!(value, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}
pub(super) fn string_or_empty(value: &Value) -> Result<String, String> {
    crate::visual_request_json::checked_string_like_model(value)
}
pub(super) fn clip(value: &str, count: usize) -> String {
    String::from_utf16_lossy(&value.encode_utf16().take(count).collect::<Vec<_>>())
}
pub(super) fn action(value: Option<&Value>) -> Result<String, String> {
    if value.is_some_and(truthy) {
        string_or_empty(value.unwrap())
    } else {
        Ok("inspect".into())
    }
}
pub(super) fn text(value: Option<&Value>) -> Result<String, String> {
    string_or_empty(value.unwrap_or(&Value::Null)).map(|value| clip(&value, 5000))
}
pub(super) fn full_page(value: Option<&Value>) -> bool {
    value != Some(&Value::Bool(false))
}
pub(super) fn idle_ms(value: Option<&Value>) -> Result<f64, String> {
    if value.is_none() {
        return Ok(600_000.0);
    }
    let number = number(value)?;
    Ok(if number.is_nan() || number == 0.0 {
        0.0
    } else {
        number.max(0.0)
    })
}
pub(super) fn json_number(number: f64) -> Value {
    if number.is_finite() && number.fract() == 0.0 && number >= 0.0 && number < u64::MAX as f64 {
        serde_json::json!(number as u64)
    } else {
        serde_json::json!(number)
    }
}
fn number(value: Option<&Value>) -> Result<f64, String> {
    let Some(value) = value else {
        return Ok(f64::NAN);
    };
    match value {
        Value::Null => return Ok(0.0),
        Value::Bool(value) => return Ok(if *value { 1.0 } else { 0.0 }),
        Value::Number(value) => return Ok(value.as_f64().unwrap_or(f64::NAN)),
        _ => {}
    }
    let text = string_or_empty(value)?;
    let text = text.trim_matches(whitespace);
    if text.is_empty() {
        return Ok(0.0);
    }
    if matches!(text, "Infinity" | "+Infinity") {
        return Ok(f64::INFINITY);
    }
    if text == "-Infinity" {
        return Ok(f64::NEG_INFINITY);
    }
    for (prefix, base) in [
        ("0x", 16_u32),
        ("0X", 16),
        ("0b", 2),
        ("0B", 2),
        ("0o", 8),
        ("0O", 8),
    ] {
        if let Some(digits) = text.strip_prefix(prefix) {
            if digits.is_empty() {
                return Ok(f64::NAN);
            }
            let mut value = 0.0;
            for char in digits.chars() {
                let Some(digit) = char.to_digit(base) else {
                    return Ok(f64::NAN);
                };
                value = value * base as f64 + digit as f64;
            }
            return Ok(value);
        }
    }
    // Rust 的浮点解析也接受 inf/NaN；JS Number 的十进制语法不接受这些拼写。
    if !text.bytes().any(|byte| byte.is_ascii_digit())
        || text
            .bytes()
            .any(|byte| !matches!(byte, b'0'..=b'9' | b'+' | b'-' | b'.' | b'e' | b'E'))
    {
        return Ok(f64::NAN);
    }
    Ok(text.parse::<f64>().unwrap_or(f64::NAN))
}
pub(super) fn dimension(
    value: Option<&Value>,
    fallback: u32,
    min: u32,
    max: u32,
) -> Result<u32, String> {
    let number = number(value)?;
    if !number.is_finite() {
        return Ok(fallback);
    }
    // Math.round 的负半数向正无穷舍入；Rust round 的负半数向负无穷。
    Ok((number + 0.5).floor().max(min as f64).min(max as f64) as u32)
}
pub(super) fn wait_ms(value: Option<&Value>, default: f64) -> Result<f64, String> {
    let number = number(value)?;
    let number = if number.is_nan() || number == 0.0 {
        default
    } else {
        number
    };
    Ok(number.max(0.0).min(15_000.0))
}
