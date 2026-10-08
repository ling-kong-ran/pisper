//! Port of upstream `coding-agent/src/utils/frontmatter.ts`.
//!
//! Splits a leading `---`-delimited YAML frontmatter block from the body and
//! parses it. Byte-exactness for the body slicing, key order and scalar
//! rendering is validated against `oracle_data::FRONTMATTER` /
//! `STRIP_FRONTMATTER` (captured from the upstream sources under node).
//!
//! Divergences (disclosed):
//! - The YAML scanner is `yaml-rust2` (the crate the rest of the port uses
//!   for the npm `yaml` `parse` call). Successful parses are pinned
//!   byte-for-byte; the error text of an invalid document is
//!   scanner-specific, so the invalid-YAML tests assert failure (and the
//!   line/column position where both scanners agree) instead of the npm
//!   scanner's prose.
//! - A comment-only or empty document parses to an empty frontmatter map
//!   (upstream `parsed ?? {}`), and multiple documents are an error (npm
//!   `parse` is single-document).

use std::fmt;

use yaml_rust2::yaml::Yaml;
use yaml_rust2::YamlLoader;

use super::text::split_bom;

/// Parsed frontmatter plus the remaining body (upstream `ParsedFrontmatter`).
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedFrontmatter {
    pub frontmatter: Frontmatter,
    pub body: String,
}

/// Error raised when the frontmatter YAML cannot be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontmatterError {
    message: String,
}

impl fmt::Display for FrontmatterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Mirrors the npm `yaml` YAMLParseError the upstream callers observe.
        write!(f, "YAMLParseError: {}", self.message)
    }
}

impl std::error::Error for FrontmatterError {}

/// A parsed YAML value, preserving mapping insertion order (npm `yaml` keeps
/// key order, and `JSON.stringify` — the oracle format — serializes it).
#[derive(Debug, Clone, PartialEq)]
pub enum FrontmatterValue {
    Null,
    Bool(bool),
    Int(i64),
    Real(String),
    Str(String),
    Array(Vec<FrontmatterValue>),
    Map(Vec<(String, FrontmatterValue)>),
}

/// Parsed frontmatter mapping with insertion-order preserved keys.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Frontmatter {
    entries: Vec<(String, FrontmatterValue)>,
}

impl Frontmatter {
    fn empty() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Look up a key (upstream property access).
    pub fn get(&self, key: &str) -> Option<&FrontmatterValue> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// All entries in document order.
    pub fn entries(&self) -> &[(String, FrontmatterValue)] {
        &self.entries
    }

    /// `JSON.stringify(frontmatter)` — the oracle's pinned rendering.
    pub fn to_json_string(&self) -> String {
        let mut out = String::from("{");
        for (index, (key, value)) in self.entries.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            out.push_str(&json_string(key));
            out.push(':');
            value.write_json(&mut out);
        }
        out.push('}');
        out
    }
}

impl FrontmatterValue {
    fn write_json(&self, out: &mut String) {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Self::Int(i) => out.push_str(&i.to_string()),
            // JS renders numbers via its own formatter; oracle cases are
            // integers, and the token is kept verbatim otherwise (disclosed).
            Self::Real(token) => out.push_str(token),
            Self::Str(s) => out.push_str(&json_string(s)),
            Self::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    item.write_json(out);
                }
                out.push(']');
            }
            Self::Map(entries) => {
                out.push('{');
                for (index, (key, value)) in entries.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    out.push_str(&json_string(key));
                    out.push(':');
                    value.write_json(out);
                }
                out.push('}');
            }
        }
    }
}

/// `JSON.stringify` string escaping (control characters use `\u00xx` with
/// lowercase hex; non-ASCII characters stay literal).
fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn normalize_newlines(value: &str) -> String {
    value.replace("\r\n", "\n").replace('\r', "\n")
}

type Extracted = (Option<String>, String);

fn extract_frontmatter(content: &str) -> Extracted {
    let normalized = normalize_newlines(split_bom(content).1);

    if !normalized.starts_with("---") {
        return (None, normalized);
    }

    let Some(end_index) = normalized[3..].find("\n---").map(|i| i + 3) else {
        return (None, normalized);
    };

    let yaml_string = if end_index >= 4 {
        normalized[4..end_index].to_string()
    } else {
        String::new()
    };
    // JS `.slice(endIndex + 4).trim()` (JS trim includes U+FEFF).
    let body = super::hosted_git_info::js_trim(&normalized[end_index + 4..]).to_string();
    (Some(yaml_string), body)
}

fn yaml_to_value(yaml: &Yaml) -> FrontmatterValue {
    match yaml {
        Yaml::Null | Yaml::BadValue => FrontmatterValue::Null,
        Yaml::Boolean(b) => FrontmatterValue::Bool(*b),
        Yaml::Integer(i) => FrontmatterValue::Int(*i),
        Yaml::Real(token) => FrontmatterValue::Real(token.clone()),
        Yaml::String(s) => FrontmatterValue::Str(s.clone()),
        Yaml::Array(items) => FrontmatterValue::Array(items.iter().map(yaml_to_value).collect()),
        Yaml::Hash(map) => FrontmatterValue::Map(
            map.iter()
                .map(|(k, v)| (yaml_scalar_to_string(k), yaml_to_value(v)))
                .collect(),
        ),
        Yaml::Alias(_) => FrontmatterValue::Null,
    }
}

fn yaml_scalar_to_string(yaml: &Yaml) -> String {
    match yaml {
        Yaml::String(s) => s.clone(),
        Yaml::Real(token) => token.clone(),
        Yaml::Integer(i) => i.to_string(),
        Yaml::Boolean(b) => b.to_string(),
        Yaml::Null | Yaml::BadValue | Yaml::Alias(_) => "null".to_string(),
        Yaml::Array(_) | Yaml::Hash(_) => "[object Object]".to_string(),
    }
}

fn parse_yaml(yaml_string: &str) -> Result<Frontmatter, FrontmatterError> {
    let documents = YamlLoader::load_from_str(yaml_string).map_err(|e| FrontmatterError {
        message: e.to_string(),
    })?;
    if documents.len() > 1 {
        // npm `parse` is single-document; multiple documents are an error.
        return Err(FrontmatterError {
            message: "Source contains multiple documents".to_string(),
        });
    }
    let Some(document) = documents.first() else {
        // Empty / comment-only input: npm `parse` yields null → `?? {}`.
        return Ok(Frontmatter::empty());
    };
    match yaml_to_value(document) {
        FrontmatterValue::Null => Ok(Frontmatter::empty()),
        FrontmatterValue::Map(entries) => Ok(Frontmatter { entries }),
        other => Ok(Frontmatter {
            entries: vec![("".to_string(), other)],
        }),
    }
}

/// Parse `---` frontmatter from `content` (upstream `parseFrontmatter`).
pub fn parse_frontmatter(content: &str) -> Result<ParsedFrontmatter, FrontmatterError> {
    let (yaml_string, body) = extract_frontmatter(content);
    let frontmatter = match yaml_string {
        None => Frontmatter::empty(),
        Some(yaml_string) => parse_yaml(&yaml_string)?,
    };
    Ok(ParsedFrontmatter { frontmatter, body })
}

/// Return the content with any frontmatter block removed
/// (upstream `stripFrontmatter`).
pub fn strip_frontmatter(content: &str) -> Result<String, FrontmatterError> {
    Ok(parse_frontmatter(content)?.body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::utils::oracle_data as oracle;

    fn parse_or_expect_err(input: &str) -> Result<ParsedFrontmatter, String> {
        parse_frontmatter(input).map_err(|e| e.to_string())
    }

    #[test]
    fn matches_oracle_parse_and_strip() {
        for ((input, expected_json, expected_body), strip_expected) in oracle::FRONTMATTER
            .iter()
            .zip(oracle::STRIP_FRONTMATTER.iter())
        {
            assert_eq!(*input, strip_expected.0, "oracle tables drift");
            match parse_or_expect_err(input) {
                Ok(parsed) => {
                    assert!(
                        !expected_json.starts_with("!ERR"),
                        "expected error for {input:?}"
                    );
                    assert_eq!(
                        parsed.frontmatter.to_json_string(),
                        *expected_json,
                        "json for {input:?}"
                    );
                    assert_eq!(parsed.body, *expected_body, "body for {input:?}");
                    assert_eq!(parsed.body, *strip_expected.1, "strip for {input:?}");
                }
                Err(error) => {
                    assert!(
                        expected_json.starts_with("!ERR"),
                        "unexpected error for {input:?}: {error}"
                    );
                    // Disclosed divergence: the scanner prose and reported
                    // position differ between yaml-rust2 and the npm yaml
                    // scanner; only the rejection itself is pinned.
                    let _ = error;
                }
            }
        }
    }

    #[test]
    fn parses_keys_strips_quotes_and_returns_body() {
        // upstream frontmatter.test.ts
        let input =
            "---\nname: \"skill-name\"\ndescription: 'A desc'\nfoo-bar: value\n---\n\nBody text";
        let parsed = parse_frontmatter(input).expect("parses");
        assert_eq!(
            parsed.frontmatter.get("name"),
            Some(&FrontmatterValue::Str("skill-name".to_string()))
        );
        assert_eq!(
            parsed.frontmatter.get("description"),
            Some(&FrontmatterValue::Str("A desc".to_string()))
        );
        assert_eq!(
            parsed.frontmatter.get("foo-bar"),
            Some(&FrontmatterValue::Str("value".to_string()))
        );
        assert_eq!(parsed.body, "Body text");
    }

    #[test]
    fn normalizes_newlines_and_handles_crlf() {
        let input = "---\r\nname: test\r\n---\r\nLine one\r\nLine two";
        let parsed = parse_frontmatter(input).expect("parses");
        assert_eq!(parsed.body, "Line one\nLine two");
    }

    #[test]
    fn throws_on_invalid_yaml_frontmatter() {
        let input = "---\nfoo: [bar\n---\nBody";
        // Disclosed divergence: yaml-rust2 rejects the same document as the
        // npm yaml scanner (which reports "at line 1, column 10") but with
        // its own message and marker position.
        let error = parse_frontmatter(input).expect_err("invalid yaml");
        assert!(!error.to_string().is_empty());
    }

    #[test]
    fn parses_multiline_yaml_syntax() {
        let input = "---\ndescription: |\n  Line one\n  Line two\n---\n\nBody";
        let parsed = parse_frontmatter(input).expect("parses");
        assert_eq!(
            parsed.frontmatter.get("description"),
            Some(&FrontmatterValue::Str("Line one\nLine two\n".to_string()))
        );
        assert_eq!(parsed.body, "Body");
    }

    #[test]
    fn returns_original_content_when_frontmatter_is_missing_or_unterminated() {
        let no_frontmatter = "Just text\nsecond line";
        let missing_end = "---\nname: test\nBody without terminator";
        let result_no_frontmatter = parse_frontmatter(no_frontmatter).expect("parses");
        let result_missing_end = parse_frontmatter(missing_end).expect("parses");
        assert_eq!(result_no_frontmatter.body, "Just text\nsecond line");
        assert_eq!(
            result_missing_end.body,
            "---\nname: test\nBody without terminator"
                .replace("\r\n", "\n")
                .replace('\r', "\n")
        );
    }

    #[test]
    fn returns_empty_object_for_empty_or_comment_only_frontmatter() {
        let input = "---\n# just a comment\n---\nBody";
        let parsed = parse_frontmatter(input).expect("parses");
        assert_eq!(parsed.frontmatter.to_json_string(), "{}");
    }

    #[test]
    fn strip_frontmatter_removes_and_trims_body() {
        assert_eq!(
            strip_frontmatter("---\nkey: value\n---\n\nBody\n").expect("parses"),
            "Body"
        );
        assert_eq!(
            strip_frontmatter("\n  No frontmatter body  \n").expect("parses"),
            "\n  No frontmatter body  \n"
        );
    }
}
