//! Ordered JSON value tree with JS `JSON.stringify` semantics.
//!
//! The upstream generator emits plain JS objects, so serialization order is
//! insertion order and integral doubles print without a fraction. To stay
//! byte-identical given the same inputs, every emitted object is a [`JsObj`]
//! (insertion-ordered) and every number goes through [`js_number`].

/// A JSON value whose objects preserve insertion order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Jv {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Jv>),
    Obj(JsObj),
}

impl From<JsObj> for Jv {
    fn from(value: JsObj) -> Self {
        Jv::Obj(value)
    }
}

impl Jv {
    pub(crate) fn s(value: impl Into<String>) -> Jv {
        Jv::Str(value.into())
    }

    pub(crate) fn n(value: f64) -> Jv {
        Jv::Num(value)
    }

    pub(crate) fn b(value: bool) -> Jv {
        Jv::Bool(value)
    }

    pub(crate) fn str_list(values: &[&str]) -> Jv {
        Jv::Arr(values.iter().map(|value| Jv::s(*value)).collect())
    }

    /// Upstream `isPlainEmptyObject` (generate-models.ts:747-749).
    pub(crate) fn is_plain_empty_object(&self) -> bool {
        matches!(self, Jv::Obj(obj) if obj.entries.is_empty())
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Jv::Str(text) => Some(text),
            _ => None,
        }
    }

    pub(crate) fn as_f64(&self) -> Option<f64> {
        match self {
            Jv::Num(value) => Some(*value),
            _ => None,
        }
    }

    /// Object member read (`value["key"]`); `None` on non-objects.
    pub(crate) fn get(&self, key: &str) -> Option<&Jv> {
        match self {
            Jv::Obj(object) => object.get(key),
            _ => None,
        }
    }
}

/// Insertion-ordered string-keyed object with JS plain-object assignment
/// semantics: [`JsObj::set`] overwrites existing keys in place and appends new
/// keys, exactly like assigning a property on a JS object (and like the tail
/// of an object spread).
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct JsObj {
    pub(crate) entries: Vec<(String, Jv)>,
}

/// Object literal builder: `obj!{ "id" => Jv::s("x"), "reasoning" => Jv::b(false) }`
/// (literal keys) and `object!{ "id" => dynamic, ... }` (expression keys).
macro_rules! obj {
    ($($key:literal => $value:expr),* $(,)?) => {{
        let mut object = JsObj::new();
        $(object.set($key, $value);)*
        object
    }};
}

macro_rules! object {
    ($($key:expr => $value:expr),* $(,)?) => {{
        let mut object = JsObj::new();
        $(object.set($key, $value);)*
        object
    }};
}

pub(crate) use obj;
pub(crate) use object;

impl JsObj {
    pub(crate) fn new() -> JsObj {
        JsObj {
            entries: Vec::new(),
        }
    }

    pub(crate) fn from_pairs(pairs: Vec<(impl Into<String>, Jv)>) -> JsObj {
        let mut object = JsObj::new();
        for (key, value) in pairs {
            object.set(&key.into(), value);
        }
        object
    }

    /// JS assignment semantics: overwrite in place, append when new.
    pub(crate) fn set(&mut self, key: &str, value: Jv) -> &mut JsObj {
        match self
            .entries
            .iter_mut()
            .find(|(existing, _)| existing == key)
        {
            Some((_, slot)) => *slot = value,
            None => self.entries.push((key.to_string(), value)),
        }
        self
    }

    pub(crate) fn get(&self, key: &str) -> Option<&Jv> {
        self.entries
            .iter()
            .find(|(existing, _)| existing == key)
            .map(|(_, value)| value)
    }

    pub(crate) fn get_mut(&mut self, key: &str) -> Option<&mut Jv> {
        self.entries
            .iter_mut()
            .find(|(existing, _)| existing == key)
            .map(|(_, value)| value)
    }

    /// JS `{...self, ...other}`: `self`'s entries keep their positions with
    /// `other`'s values winning, then `other`'s new keys append in order.
    pub(crate) fn spread(&mut self, other: &JsObj) -> &mut JsObj {
        for (key, value) in &other.entries {
            self.set(key, value.clone());
        }
        self
    }

    /// JS `delete obj.key`.
    pub(crate) fn remove(&mut self, key: &str) {
        self.entries.retain(|(existing, _)| existing != key);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Upstream `serializeJson` (generate-models.ts:3061): compact
/// `JSON.stringify` + trailing newline (the committed data files and manifest
/// are compact).
pub(crate) fn serialize_json(value: &Jv) -> String {
    let mut out = String::new();
    write_jv(value, &mut out);
    out.push('\n');
    out
}

fn write_jv(value: &Jv, out: &mut String) {
    match value {
        Jv::Null => out.push_str("null"),
        Jv::Bool(true) => out.push_str("true"),
        Jv::Bool(false) => out.push_str("false"),
        Jv::Num(number) => out.push_str(&js_number(*number)),
        Jv::Str(text) => write_json_string(text, out),
        Jv::Arr(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_jv(item, out);
            }
            out.push(']');
        }
        Jv::Obj(object) => {
            out.push('{');
            for (index, (key, item)) in object.entries.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_json_string(key, out);
                out.push(':');
                write_jv(item, out);
            }
            out.push('}');
        }
    }
}

/// JS `Number -> string` for JSON output: integral values (and -0) print
/// without a fraction, everything else as the shortest round-trip decimal.
/// JS switches to exponent notation only outside [1e-6, 1e21); the generator's
/// numbers (per-million-token costs, token counts) stay inside that window.
pub(crate) fn js_number(value: f64) -> String {
    if !value.is_finite() {
        // JSON.stringify(NaN/Infinity) = "null"; the transforms never produce
        // one (costs default through `|| 0`), so this is a safety net.
        return "null".to_string();
    }
    if value == 0.0 {
        return "0".to_string();
    }
    if value == value.trunc() && value.abs() < 1e21 {
        return format!("{value:.0}");
    }
    format!("{value}")
}

/// JS `JSON.stringify` string escaping: quotes, backslash, and the C0
/// controls; everything else (including non-ASCII) stays literal UTF-8.
fn write_json_string(text: &str, out: &mut String) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// JS `parseFloat` on the leading numeric prefix (`parseFloat("0.5x") = 0.5`).
/// Where JS would produce NaN the port maps to 0: pricing strings always
/// reach `parseFloat` through `|| "0"` upstream, and a NaN cost would
/// serialize as a JSON `null` instead of a number (see the module docs of the
/// binary for the disclosed corner).
pub(crate) fn js_parse_f64(text: &str) -> f64 {
    let bytes = text.as_bytes();
    let mut end = 0;
    let mut seen_digit = false;
    let mut seen_dot = false;
    let mut seen_exp = false;
    while end < bytes.len() {
        let byte = bytes[end];
        if byte.is_ascii_digit() {
            seen_digit = true;
        } else if byte == b'.' && !seen_dot && !seen_exp {
            seen_dot = true;
        } else if (byte == b'e' || byte == b'E') && seen_digit && !seen_exp {
            // An exponent needs digits afterwards; `parseFloat("1e") = 1`
            // stops here because the next iteration breaks on end-of-input,
            // and the suffix check below trims the trailing `e`.
            seen_exp = true;
        } else if (byte == b'+' || byte == b'-')
            && (end == 0 || bytes[end - 1] == b'e' || bytes[end - 1] == b'E')
        {
            if end == 0 && byte == b'+' {
                // Leading `+` is not part of parseFloat's number grammar.
                break;
            }
        } else {
            break;
        }
        end += 1;
    }
    // Trim a trailing incomplete exponent (e.g. "1e+" or "1e").
    while end > 0
        && (bytes[end - 1] == b'e'
            || bytes[end - 1] == b'E'
            || bytes[end - 1] == b'+'
            || bytes[end - 1] == b'-')
    {
        end -= 1;
    }
    if !seen_digit {
        return 0.0;
    }
    text[..end].parse::<f64>().unwrap_or(0.0)
}

/// JS `Number(value.toFixed(6))`: round to six decimal places. Both JS
/// `toFixed` and Rust's `{:.6}` round the exact decimal expansion of the
/// double, and an exact tie (which would distinguish their tie-breaking
/// rules) cannot occur for a non-terminating expansion, so the results agree;
/// -0 is normalized because `JSON.stringify(-0)` is `0`.
pub(crate) fn round_cost(value: f64) -> f64 {
    let rounded = format!("{value:.6}").parse::<f64>().unwrap_or(0.0);
    if rounded == 0.0 {
        0.0
    } else {
        rounded
    }
}

/// JS `x || 0` for numeric catalog fields: nullish and 0 become 0 (JSON
/// doubles cannot be NaN/""/false here).
pub(crate) fn js_or(value: Option<f64>, fallback: f64) -> f64 {
    match value {
        Some(value) if value != 0.0 => value,
        _ => fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obj_preserves_insertion_order_and_js_assignment_semantics() {
        let mut object = obj! { "id" => Jv::s("a"), "reasoning" => Jv::b(false) };
        // Existing key overwritten in place, new key appended.
        object
            .set("id", Jv::s("b"))
            .set("compat", Jv::Obj(obj! { "x" => Jv::b(true) }));
        assert_eq!(
            serialize_json(&Jv::Obj(object)),
            "{\"id\":\"b\",\"reasoning\":false,\"compat\":{\"x\":true}}\n"
        );
    }

    #[test]
    fn spread_matches_js_object_spread_order() {
        let mut target = obj! { "a" => Jv::n(1.0), "b" => Jv::n(2.0) };
        let other = obj! { "b" => Jv::n(20.0), "c" => Jv::n(3.0) };
        target.spread(&other);
        assert_eq!(
            serialize_json(&Jv::Obj(target)),
            "{\"a\":1,\"b\":20,\"c\":3}\n"
        );
    }

    #[test]
    fn js_number_matches_json_stringify_for_catalog_shapes() {
        assert_eq!(js_number(3.0), "3");
        assert_eq!(js_number(0.5), "0.5");
        assert_eq!(js_number(-0.0), "0");
        assert_eq!(js_number(0.0), "0");
        assert_eq!(js_number(0.0001), "0.0001");
        assert_eq!(js_number(272_000.0), "272000");
        assert_eq!(
            js_number(0.3 + 0.1),
            serialize_json(&Jv::Num(0.3 + 0.1)).trim()
        ); // shortest repr, self-consistent
    }

    #[test]
    fn round_cost_matches_to_fixed_six() {
        assert_eq!(round_cost(0.15), 0.15);
        assert_eq!(round_cost(0.1234567), 0.123457);
        assert_eq!(round_cost(-0.0000001), 0.0);
        assert_eq!(round_cost(2.0), 2.0);
    }

    #[test]
    fn js_parse_f64_takes_the_leading_prefix() {
        assert_eq!(js_parse_f64("0.00000123"), 0.00000123);
        assert_eq!(js_parse_f64("1.5e-4"), 0.00015);
        assert_eq!(js_parse_f64("12abc"), 12.0);
        assert_eq!(js_parse_f64("-0.5"), -0.5);
        assert_eq!(js_parse_f64(""), 0.0);
        assert_eq!(js_parse_f64("abc"), 0.0);
        assert_eq!(js_parse_f64("1e"), 1.0);
        assert_eq!(js_parse_f64("+2"), 0.0);
    }

    #[test]
    fn json_string_escaping_matches_json_stringify() {
        let mut out = String::new();
        write_json_string("a\"b\\c\nd\te\u{1}f", &mut out);
        assert_eq!(out, "\"a\\\"b\\\\c\\nd\\te\\u0001f\"");
    }
}
