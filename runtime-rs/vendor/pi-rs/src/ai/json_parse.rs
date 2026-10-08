//! The JSON-parse/repair port from upstream
//! `packages/ai/src/utils/json-parse.ts`, shared by the API ports that must
//! tolerate near-JSON provider payloads (the anthropic SSE event reader; the
//! streaming tool-call argument accumulator over in
//! `api::openai_completions::stream`).
//!
//! Upstream shape: `parseJsonWithRepair` parses directly, retries the
//! repaired text when the repair differs, and otherwise surfaces the original
//! error; `repairJson` escapes raw control characters inside strings and
//! doubles backslashes before invalid escape sequences. The streaming
//! companion (`parseStreamingJson`, never-throw `{}` fallback) stays beside
//! its callers since it additionally depends on the `partial-json`
//! approximation.

use serde_json::Value;

const VALID_JSON_ESCAPES: [char; 9] = ['"', '\\', '/', 'b', 'f', 'n', 'r', 't', 'u'];

/// Upstream `parseJsonWithRepair` (`utils/json-parse.ts:85-95`): direct
/// parse, then the repaired text when the repair differs, otherwise the
/// original error.
pub(crate) fn parse_json_with_repair(json: &str) -> Result<Value, serde_json::Error> {
    match serde_json::from_str(json) {
        Ok(value) => Ok(value),
        Err(error) => {
            let repaired = repair_json(json);
            if repaired != json {
                serde_json::from_str(&repaired)
            } else {
                Err(error)
            }
        }
    }
}

/// Upstream `repairJson` (`utils/json-parse.ts:39-94`): escape raw control
/// characters inside strings and double backslashes before invalid escapes.
pub(crate) fn repair_json(json: &str) -> String {
    let mut repaired = String::with_capacity(json.len());
    let mut in_string = false;
    let mut chars = json.chars().peekable();
    while let Some(current) = chars.next() {
        if !in_string {
            repaired.push(current);
            if current == '"' {
                in_string = true;
            }
            continue;
        }
        match current {
            '"' => {
                repaired.push('"');
                in_string = false;
            }
            '\\' => match chars.peek().copied() {
                None => repaired.push_str("\\\\"),
                Some(next) => {
                    if next == 'u' {
                        let digits: String = chars.clone().take(4).collect();
                        if digits.chars().count() == 4
                            && digits.chars().all(|c| c.is_ascii_hexdigit())
                        {
                            repaired.push_str("\\u");
                            repaired.push_str(&digits);
                            for _ in 0..4 {
                                chars.next();
                            }
                            continue;
                        }
                    }
                    if VALID_JSON_ESCAPES.contains(&next) {
                        repaired.push('\\');
                        repaired.push(next);
                        chars.next();
                    } else {
                        repaired.push_str("\\\\");
                    }
                }
            },
            other => {
                if (other as u32) <= 0x1f {
                    repaired.push_str(&escape_control_character(other));
                } else {
                    repaired.push(other);
                }
            }
        }
    }
    repaired
}

/// Upstream `escapeControlCharacter` (`utils/json-parse.ts:15-27`).
fn escape_control_character(character: char) -> String {
    match character {
        '\u{8}' => "\\b".to_string(),
        '\u{c}' => "\\f".to_string(),
        '\n' => "\\n".to_string(),
        '\r' => "\\r".to_string(),
        '\t' => "\\t".to_string(),
        other => format!("\\u{:04x}", other as u32),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parse_json_with_repair_parses_clean_and_repaired_input() {
        assert_eq!(
            parse_json_with_repair(r#"{"a":1}"#).unwrap(),
            json!({"a":1})
        );
        // A raw control character inside a string is repaired into an escape.
        assert_eq!(
            parse_json_with_repair("{\"a\":\"x\u{7}\"}").unwrap(),
            json!({"a":"x\u{7}"})
        );
        // When the repair cannot help (repair == input), the original error
        // surfaces.
        assert!(parse_json_with_repair("not json").is_err());
    }

    #[test]
    fn repair_json_escapes_controls_and_backslashes() {
        assert_eq!(repair_json("{\"a\":\"b\\\\c\"}"), "{\"a\":\"b\\\\c\"}");
        // An invalid escape's backslash is doubled, making it parseable.
        assert_eq!(
            parse_json_with_repair(&repair_json(r#"{"a":"x\y"}"#)).unwrap(),
            json!({"a":"x\\y"})
        );
        // Trailing lone backslash inside a string (doubled so the close is
        // not consumed; the string stays open — the streaming completion
        // layer closes it).
        assert_eq!(repair_json(r#""a\"#), r#""a\\"#);
        // A valid escaped quote passes through unchanged.
        assert_eq!(repair_json(r#""a\""#), r#""a\""#);
        // Raw newline inside a string becomes \n.
        assert_eq!(repair_json("{\"a\":\"x\ny\"}"), "{\"a\":\"x\\ny\"}");
    }
}
