//! Port of upstream `coding-agent/src/utils/text.ts` (leaf pulled in by
//! `frontmatter.ts`, which imports `stripBom`).

/// Split a leading UTF-8 byte order mark from decoded text.
pub fn split_bom(content: &str) -> (&str, &str) {
    if let Some(text) = content.strip_prefix('\u{feff}') {
        ("\u{feff}", text)
    } else {
        ("", content)
    }
}

/// Remove a leading UTF-8 byte order mark from decoded text.
pub fn strip_bom(content: &str) -> &str {
    split_bom(content).1
}

/// ECMAScript String.prototype.trim: WhiteSpace plus LineTerminator, not
/// Unicode White_Space (which would wrongly trim NEL U+0085).
pub(crate) fn trim_js_whitespace(value: &str) -> &str {
    value.trim_matches(|c| {
        matches!(c,
        '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' |
        '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' |
        '\u{205f}' | '\u{3000}' | '\u{feff}')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_and_strips_bom() {
        assert_eq!(split_bom("\u{feff}abc"), ("\u{feff}", "abc"));
        assert_eq!(split_bom("abc"), ("", "abc"));
        assert_eq!(split_bom(""), ("", ""));
        assert_eq!(split_bom("\u{feff}"), ("\u{feff}", ""));
        assert_eq!(strip_bom("\u{feff}x"), "x");
        assert_eq!(strip_bom("x"), "x");
    }
}
