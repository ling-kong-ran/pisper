//! Port of upstream `coding-agent/src/utils/json.ts`.
//!
//! Strip `//` line comments and trailing commas from JSON, leaving string
//! literals untouched.
//!
//! Byte-exactness validated against the oracle in [`oracle_data`]. The regex
//! character classes replicate JavaScript semantics exactly: `\\.` must not
//! match `\r`/`\u{2028}`/`\u{2029}` (JS `.` excludes them, Rust's `.` only
//! excludes `\n`), and `\s` is spelled out because Rust's `\s` (Unicode
//! White_Space) does not include `\u{FEFF}`.

use std::sync::OnceLock;

use regex::{Captures, Regex};

const JS_DOT: &str = r"[^\n\r\u{2028}\u{2029}]";
// JavaScript `\s`: White_Space plus U+FEFF.
const JS_WS: &str =
    r"[\t\n\v\f\r \u{a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}]";

fn comment_or_string_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(r#"("(?:(?:\\{JS_DOT})|[^"\\])*")|(//[^\n]*)"#)).expect("valid")
    })
}

fn string_or_comma_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(
            r#"("(?:(?:\\{JS_DOT})|[^"\\])*")|(,({JS_WS}*[}}\]]))"#
        ))
        .expect("valid")
    })
}

/// Strip `//` line comments and trailing commas from JSON, leaving string
/// literals untouched.
pub fn strip_json_comments(input: &str) -> String {
    let without_comments = comment_or_string_regex().replace_all(input, |c: &Captures| {
        if c.get(2).is_some() {
            String::new()
        } else {
            c[0].to_string()
        }
    });
    string_or_comma_regex()
        .replace_all(&without_comments, |c: &Captures| {
            // Upstream: (m, tail) => tail ?? (m[0] === '"' ? m : "")
            if c.get(2).is_some() {
                c[3].to_string()
            } else {
                c[0].to_string()
            }
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::utils::oracle_data as oracle;

    #[test]
    fn matches_upstream_byte_for_byte() {
        for (input, expected) in oracle::STRIP_JSON_COMMENTS {
            assert_eq!(strip_json_comments(input), *expected, "input: {input:?}");
        }
    }

    #[test]
    fn preserves_valid_json() {
        let original = r#"{"a": [1, 2], "b": {"c": "//x"}}"#;
        assert_eq!(strip_json_comments(original), original);
    }
}
