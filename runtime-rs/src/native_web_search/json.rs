//! Web-search-only JSON input adaptation to the release's JSON.parse and
//! outgoing lone-surrogate cleaning. Structural JSON validation stays strict.
use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

fn unicode_escape(input: &[u8], index: usize) -> Option<u16> {
    let bytes = input.get(index..index + 6)?;
    if &bytes[..2] != b"\\u" {
        return None;
    }
    u16::from_str_radix(std::str::from_utf8(&bytes[2..]).ok()?, 16).ok()
}

pub(super) fn parse(input: &str) -> Result<Value, serde_json::Error> {
    let input = input.as_bytes();
    let mut output = Vec::with_capacity(input.len());
    let mut index = 0;
    let mut quoted = false;
    let mut containers = Vec::new();
    let mut previous = None;
    while index < input.len() {
        let byte = input[index];
        if quoted {
            if byte == b'\\' {
                if let Some(unit) = unicode_escape(input, index) {
                    if (0xd800..=0xdbff).contains(&unit)
                        && unicode_escape(input, index + 6)
                            .is_some_and(|low| (0xdc00..=0xdfff).contains(&low))
                    {
                        output.extend_from_slice(&input[index..index + 12]);
                        index += 12;
                        continue;
                    }
                    if (0xd800..=0xdfff).contains(&unit) {
                        output.extend_from_slice(b"\\ufffd");
                        index += 6;
                        continue;
                    }
                }
                // An escaped backslash must not turn a following literal
                // "ud800" into a Unicode escape. Invalid escapes remain invalid.
                let end = (index + 2).min(input.len());
                output.extend_from_slice(&input[index..end]);
                index = end;
                continue;
            }
            if byte == b'"' {
                quoted = false;
                previous = Some(b'"');
            }
        } else {
            if byte == b'"' {
                quoted = true;
            } else if matches!(byte, b'{' | b'[') {
                containers.push(byte);
            } else if matches!(byte, b'}' | b']') {
                containers.pop();
            }
            let value_position = previous.is_none()
                || matches!(previous, Some(b':' | b'['))
                || (previous == Some(b',') && containers.last() == Some(&b'['));
            if value_position && matches!(byte, b'-' | b'0'..=b'9') {
                let mut end = index + 1;
                while input.get(end).is_some_and(|value| {
                    matches!(value, b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
                }) {
                    end += 1;
                }
                let token = std::str::from_utf8(&input[index..end]).expect("ASCII number");
                static NUMBER: OnceLock<Regex> = OnceLock::new();
                let number = NUMBER.get_or_init(|| {
                    Regex::new(r"^-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?$")
                        .expect("JSON number grammar")
                });
                if number.is_match(token)
                    && token.parse::<f64>().is_ok_and(|value| value.is_infinite())
                {
                    // Search's String/Number/truthiness coercions observe
                    // +/-Infinity like these strings. This adapted document
                    // is read-only and never replaces canonical JSON bytes.
                    output.extend_from_slice(if token.starts_with('-') {
                        b"\"-Infinity\""
                    } else {
                        b"\"Infinity\""
                    });
                } else {
                    output.extend_from_slice(&input[index..end]);
                }
                index = end;
                previous = Some(b'0');
                continue;
            }
            if !matches!(byte, b' ' | b'\n' | b'\r' | b'\t') {
                previous = Some(byte);
            }
        }
        output.push(byte);
        index += 1;
    }
    serde_json::from_slice(&output)
}
