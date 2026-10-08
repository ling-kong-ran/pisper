//! Port of upstream `coding-agent/src/utils/ansi.ts` (itself derived from
//! chalk's ansi-regex / strip-ansi, MIT, (c) Sindre Sorhus).
//!
//! Byte-exactness validated against the oracle in [`oracle_data`].
//!
//! Divergence: the upstream TypeError branch for non-string values does not
//! exist in Rust's typed signature (`&str`).

use std::borrow::Cow;
use std::sync::OnceLock;

use regex::Regex;

// OSC sequences only: ESC ] ... ST (non-greedy until the first terminator;
// valid string terminators are BEL `\u{7}`, ESC\ `\x1B\x5C`, and 0x9c
// `\x{9C}`)
const OSC: &str = r"(?:\x1B\][\s\S]*?(?:\u{7}|\x1B\x5C|\x{9C}))";
// CSI and related: ESC/C1, optional intermediates, optional params
// (supports ; and :) then final byte
const CSI: &str = r"[\x1B\x{9B}][\[\]()#;?]*(?:\d{1,4}(?:[;:]\d{0,4})*)?[\dA-PR-TZcf-nq-uy=><~]";

fn ansi_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(&format!("{OSC}|{CSI}")).expect("ansi pattern is valid"))
}

/// Strip ANSI escape sequences, returning the input unchanged when it
/// contains no introducer bytes (upstream fast path).
pub fn strip_ansi(value: &str) -> Cow<'_, str> {
    // Fast path: ANSI codes require ESC (7-bit) or CSI (8-bit) introducer
    if !value.contains('\u{1b}') && !value.contains('\u{9b}') {
        return Cow::Borrowed(value);
    }
    Cow::Owned(ansi_regex().replace_all(value, "").into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::utils::oracle_data as oracle;

    #[test]
    fn matches_chalk_strip_ansi_for_generated_compatibility_inputs() {
        for (input, expected) in oracle::STRIP_ANSI_COMPAT {
            assert_eq!(strip_ansi(input), *expected, "mismatch for input {input:?}");
        }
    }

    #[test]
    fn strips_common_ansi_sequences_used_in_tool_output() {
        assert_eq!(strip_ansi(oracle::STRIP_ANSI_TOOL_OUTPUT), "aredlinkz");
    }

    #[test]
    fn strips_single_byte_esc_sequences_without_leaking_final_bytes() {
        for code in b'g'..=b'm' {
            let input = format!("\x1b{}ok", code as char);
            assert_eq!(strip_ansi(&input), "ok");
        }
        for code in b'r'..=b't' {
            let input = format!("\x1b{}ok", code as char);
            assert_eq!(strip_ansi(&input), "ok");
        }
    }

    #[test]
    fn strips_ris_without_leaking_the_final_byte() {
        assert_eq!(strip_ansi("\x1bcdone"), "done");
    }

    #[test]
    fn fast_path_returns_borrowed_value() {
        let plain = "plain text";
        assert!(matches!(strip_ansi(plain), Cow::Borrowed(_)));
    }

    #[test]
    fn strips_inside_bom_prefixed_text() {
        assert_eq!(strip_ansi(oracle::STRIP_ANSI_BOM_AND_COLOR), "\u{feff}J");
    }
}
