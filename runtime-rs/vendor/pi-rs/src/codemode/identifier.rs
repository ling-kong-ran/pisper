//! Port of upstream `codemode/src/identifier.ts` (verbatim logic).
//!
//! The identifier a script uses for a tool: characters that are not valid in
//! a JavaScript identifier become `_`. `mcp__docs__search` stays as is,
//! `my-tool` becomes `my_tool`.

/// Upstream `toCodemodeIdentifier`. Iterates Unicode code points (the
/// upstream loop is `for (const char of name)`, code-point-wise), so a
/// non-ASCII letter such as `é` fails the first/continuation test and
/// becomes `_`, exactly one underscore per code point.
pub fn to_codemode_identifier(name: &str) -> String {
    let mut identifier = String::with_capacity(name.len());
    for (index, char) in name.chars().enumerate() {
        let valid = if index == 0 {
            char.is_ascii_alphabetic() || char == '_' || char == '$'
        } else {
            char.is_ascii_alphanumeric() || char == '_' || char == '$'
        };
        if valid {
            identifier.push(char);
        } else {
            identifier.push('_');
        }
    }
    if identifier.is_empty() {
        return "_".to_string();
    }
    identifier
}

#[cfg(test)]
mod tests {
    use super::to_codemode_identifier;

    #[test]
    fn normalizes_to_identifiers() {
        assert_eq!(
            to_codemode_identifier("mcp__docs__search"),
            "mcp__docs__search"
        );
        assert_eq!(to_codemode_identifier("my-tool"), "my_tool");
        assert_eq!(to_codemode_identifier("2fast"), "_fast");
        assert_eq!(to_codemode_identifier(""), "_");
        assert_eq!(to_codemode_identifier("$x"), "$x");
        assert_eq!(to_codemode_identifier("é"), "_");
    }
}
