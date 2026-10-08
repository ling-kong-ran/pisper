//! Port of upstream `codemode/src/source.ts` (verbatim logic) — the codemode
//! source format: JavaScript, optionally preceded by one options line.
//!
//! ```js
//! // @options: {"max_output_tokens": 2000, "timeout_ms": 30000}
//! const text = await tools.read({ path: "package.json" });
//! ```
//!
//! Disclosed divergence: the invalid-JSON error text embeds the host JSON
//! parser's message (upstream `JSON.parse`, V8; here `serde_json`), so the
//! `@options must be valid JSON …:` suffix differs for malformed JSON.

/// Upstream `CODEMODE_OPTIONS_PREFIX`.
pub const CODEMODE_OPTIONS_PREFIX: &str = "// @options:";

/// Upstream `CODEMODE_SOURCE_GRAMMAR` — the Lark grammar handed to
/// grammar-constrained providers. Byte-exact, including the leading newline.
pub const CODEMODE_SOURCE_GRAMMAR: &str = r"
start: options_source | plain_source
options_source: OPTIONS_LINE NEWLINE SOURCE
plain_source: SOURCE

OPTIONS_LINE: /[ \t]*\/\/ @options:[^\r\n]*/
NEWLINE: /\r?\n/
SOURCE: /[\s\S]+/
";

/// Upstream `MAX_TIMEOUT_MS`: largest delay `setTimeout` supports, which
/// bounds `timeout_ms`.
const MAX_TIMEOUT_MS: f64 = 2_147_483_647.0;

const SUPPORTED_FIELDS_TEXT: &str = "`max_output_tokens` and `timeout_ms`";

/// Upstream `CodemodeSourceOptions`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CodemodeSourceOptions {
    /// Token budget for the script's output.
    pub max_output_tokens: Option<f64>,
    /// Hard deadline for the whole script in milliseconds.
    pub timeout_ms: Option<u64>,
}

/// Upstream `ParsedCodemodeSource`. The code has the options line replaced by
/// an empty line, so line numbers are unchanged.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedCodemodeSource {
    pub code: String,
    pub options: CodemodeSourceOptions,
}

/// Upstream `CodemodeSourceError`; `Display` produces the message.
#[derive(Debug, Clone, PartialEq)]
pub struct CodemodeSourceError(pub String);

impl std::fmt::Display for CodemodeSourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CodemodeSourceError {}

/// Upstream `isSafeInteger(value) && value >= 0`.
fn is_non_negative_safe_integer(value: &serde_json::Value) -> bool {
    let Some(number) = value.as_f64() else {
        return false;
    };
    number.is_finite() && number.fract() == 0.0 && (0.0..=9_007_199_254_740_991.0).contains(&number)
}

fn parse_options(directive: &str) -> Result<CodemodeSourceOptions, CodemodeSourceError> {
    let unsupported = |key: &str| {
        CodemodeSourceError(format!(
            "@options only supports {SUPPORTED_FIELDS_TEXT}; got `{key}`"
        ))
    };
    if directive.is_empty() {
        return Err(CodemodeSourceError(format!(
            "@options must be a JSON object with supported fields {SUPPORTED_FIELDS_TEXT}"
        )));
    }
    let value: serde_json::Value = serde_json::from_str(directive).map_err(|error| {
        CodemodeSourceError(format!(
            "@options must be valid JSON with supported fields {SUPPORTED_FIELDS_TEXT}: {error}"
        ))
    })?;
    let Some(fields) = value.as_object() else {
        return Err(CodemodeSourceError(format!(
            "@options must be a JSON object with supported fields {SUPPORTED_FIELDS_TEXT}"
        )));
    };
    let mut options = CodemodeSourceOptions::default();
    // `for (const key of Object.keys(fields))`: insertion order
    // (`preserve_order` keeps the document order).
    for (key, _) in fields {
        if key != "max_output_tokens" && key != "timeout_ms" {
            return Err(unsupported(key));
        }
    }
    if let Some(max_output_tokens) = fields.get("max_output_tokens") {
        if !is_non_negative_safe_integer(max_output_tokens) {
            return Err(CodemodeSourceError(
                "@options field `max_output_tokens` must be a non-negative safe integer"
                    .to_string(),
            ));
        }
        options.max_output_tokens = max_output_tokens.as_f64();
    }
    if let Some(timeout_ms) = fields.get("timeout_ms") {
        let valid = is_non_negative_safe_integer(timeout_ms)
            && timeout_ms.as_f64().is_some_and(|value| value != 0.0)
            && timeout_ms
                .as_f64()
                .is_some_and(|value| value <= MAX_TIMEOUT_MS);
        if !valid {
            return Err(CodemodeSourceError(format!(
                "@options field `timeout_ms` must be a positive integer up to {MAX_TIMEOUT_MS}"
            )));
        }
        // A validated safe positive integer at most `MAX_TIMEOUT_MS` fits u64.
        options.timeout_ms = timeout_ms.as_f64().map(|value| value as u64);
    }
    Ok(options)
}

/// Upstream `parseCodemodeSource`: split an optional first-line
/// `// @options: {...}` from the script. Errors for empty input and invalid
/// options.
pub fn parse_codemode_source(input: &str) -> Result<ParsedCodemodeSource, CodemodeSourceError> {
    if input.trim().is_empty() {
        return Err(CodemodeSourceError(
            "Expected JavaScript source text (non-empty). Provide JS only, optionally with a first line `// @options: {\"max_output_tokens\": 1000}`."
                .to_string(),
        ));
    }
    let newline = input.find('\n');
    let first_line = match newline {
        Some(index) => &input[..index],
        None => input,
    };
    let first_line = first_line.strip_suffix('\r').unwrap_or(first_line);
    let trimmed = first_line.trim_start();
    if !trimmed.starts_with(CODEMODE_OPTIONS_PREFIX) {
        return Ok(ParsedCodemodeSource {
            code: input.to_string(),
            options: CodemodeSourceOptions::default(),
        });
    }
    let code = match newline {
        Some(index) => input[index..].to_string(),
        None => String::new(),
    };
    if code.trim().is_empty() {
        return Err(CodemodeSourceError(
            "The @options line must be followed by JavaScript source on subsequent lines"
                .to_string(),
        ));
    }
    Ok(ParsedCodemodeSource {
        code,
        options: parse_options(trimmed[CODEMODE_OPTIONS_PREFIX.len()..].trim())?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_plain_source_through() {
        let parsed = parse_codemode_source("return 1;\n").unwrap();
        assert_eq!(parsed.code, "return 1;\n");
        assert_eq!(parsed.options, CodemodeSourceOptions::default());
    }

    #[test]
    fn parses_options_line() {
        let parsed = parse_codemode_source(
            "// @options: {\"max_output_tokens\": 2000, \"timeout_ms\": 30000}\nreturn 1;",
        )
        .unwrap();
        assert_eq!(parsed.code, "\nreturn 1;");
        assert_eq!(
            parsed.options,
            CodemodeSourceOptions {
                max_output_tokens: Some(2000.0),
                timeout_ms: Some(30000),
            }
        );
    }

    #[test]
    fn rejects_empty_input() {
        assert!(parse_codemode_source("   \n").is_err());
    }

    #[test]
    fn rejects_unknown_field() {
        let error = parse_codemode_source("// @options: {\"nope\": 1}\nreturn 1;").unwrap_err();
        assert_eq!(
            error.0,
            "@options only supports `max_output_tokens` and `timeout_ms`; got `nope`"
        );
    }

    #[test]
    fn rejects_zero_timeout() {
        let error =
            parse_codemode_source("// @options: {\"timeout_ms\": 0}\nreturn 1;").unwrap_err();
        assert_eq!(
            error.0,
            "@options field `timeout_ms` must be a positive integer up to 2147483647"
        );
    }

    #[test]
    fn rejects_options_without_body() {
        let error = parse_codemode_source("// @options: {}").unwrap_err();
        assert_eq!(
            error.0,
            "The @options line must be followed by JavaScript source on subsequent lines"
        );
    }
}
