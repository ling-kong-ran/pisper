//! Differential oracles generated from the ACTUAL upstream
//! `packages/tui/src/utils.ts` executed under Node (see
//! `docs/migration/reference/generate-tui-width-tables.mjs`). Expected values
//! are byte-exact upstream outputs.

use std::sync::OnceLock;

use crate::tui::utils::*;

const FIXTURE_JSON: &str = include_str!("../fixtures/width-oracles.json");

fn fixtures() -> &'static serde_json::Value {
    static FIXTURES: OnceLock<serde_json::Value> = OnceLock::new();
    FIXTURES.get_or_init(|| serde_json::from_str(FIXTURE_JSON).expect("valid width-oracles.json"))
}

#[test]
fn differential_visible_width_matches_upstream_for_the_full_sweep() {
    let fixtures = fixtures();
    let inputs = fixtures["visibleWidth"]["inputs"].as_array().unwrap();
    let expected = fixtures["visibleWidth"]["expected"].as_array().unwrap();
    assert_eq!(inputs.len(), expected.len());
    let mut mismatches = 0;
    for (input, expected) in inputs.iter().zip(expected) {
        let input = input.as_str().unwrap();
        let expected = expected.as_u64().unwrap() as usize;
        let actual = visible_width(input);
        if actual != expected {
            if mismatches < 20 {
                eprintln!("visibleWidth mismatch for {input:?}: expected {expected}, got {actual}");
            }
            mismatches += 1;
        }
    }
    assert_eq!(mismatches, 0, "visibleWidth mismatches across the sweep");
}

#[test]
fn differential_wrap_matches_upstream_byte_for_byte() {
    let fixtures = fixtures();
    let cases = fixtures["wrap"]["cases"].as_array().unwrap();
    let expected = fixtures["wrap"]["expected"].as_array().unwrap();
    assert_eq!(cases.len(), expected.len());
    for (case, expected) in cases.iter().zip(expected) {
        let input = case[0].as_str().unwrap();
        let width = case[1].as_u64().unwrap() as usize;
        let expected: Vec<&str> = expected
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l.as_str().unwrap())
            .collect();
        assert_eq!(
            wrap_text_with_ansi(input, width),
            expected,
            "wrap mismatch for {input:?} at width {width}"
        );
    }
}

#[test]
fn differential_truncate_matches_upstream_byte_for_byte() {
    let fixtures = fixtures();
    let cases = fixtures["truncate"]["cases"].as_array().unwrap();
    let expected = fixtures["truncate"]["expected"].as_array().unwrap();
    assert_eq!(cases.len(), expected.len());
    for (case, expected) in cases.iter().zip(expected) {
        let input = case[0].as_str().unwrap();
        let max_width = case[1].as_u64().unwrap() as usize;
        let ellipsis = case[2].as_str().unwrap();
        let pad = case[3].as_bool().unwrap();
        assert_eq!(
            truncate_to_width(input, max_width, ellipsis, pad),
            expected.as_str().unwrap(),
            "truncate mismatch for {input:?} at width {max_width}"
        );
    }
}

#[test]
fn differential_slice_matches_upstream() {
    let fixtures = fixtures();
    let cases = fixtures["slice"]["cases"].as_array().unwrap();
    let expected = fixtures["slice"]["expected"].as_array().unwrap();
    assert_eq!(cases.len(), expected.len());
    for (case, expected) in cases.iter().zip(expected) {
        let input = case[0].as_str().unwrap();
        let start = case[1].as_u64().unwrap() as usize;
        let length = case[2].as_u64().unwrap() as usize;
        let strict = case[3].as_bool().unwrap();
        let expected = expected.as_array().unwrap();
        let (text, width) = slice_with_width(input, start, length, strict);
        assert_eq!(
            &text,
            expected[0].as_str().unwrap(),
            "slice text mismatch for {input:?}"
        );
        assert_eq!(
            width,
            expected[1].as_u64().unwrap() as usize,
            "slice width mismatch for {input:?}"
        );
    }
}

#[test]
fn differential_extract_segments_matches_upstream() {
    let fixtures = fixtures();
    let cases = fixtures["extractSegments"]["cases"].as_array().unwrap();
    let expected = fixtures["extractSegments"]["expected"].as_array().unwrap();
    assert_eq!(cases.len(), expected.len());
    for (case, expected) in cases.iter().zip(expected) {
        let input = case[0].as_str().unwrap();
        let before_end = case[1].as_u64().unwrap() as usize;
        let after_start = case[2].as_u64().unwrap() as usize;
        let after_len = case[3].as_u64().unwrap() as usize;
        let strict_after = case[4].as_bool().unwrap();
        let expected = expected.as_array().unwrap();
        let segments = extract_segments(input, before_end, after_start, after_len, strict_after);
        assert_eq!(
            &segments.before,
            expected[0].as_str().unwrap(),
            "before mismatch for {input:?}"
        );
        assert_eq!(
            segments.before_width,
            expected[1].as_u64().unwrap() as usize
        );
        assert_eq!(
            &segments.after,
            expected[2].as_str().unwrap(),
            "after mismatch for {input:?}"
        );
        assert_eq!(segments.after_width, expected[3].as_u64().unwrap() as usize);
    }
}

#[test]
fn differential_get_grapheme_cell_range_matches_upstream() {
    let fixtures = fixtures();
    let cases = fixtures["cellRange"]["cases"].as_array().unwrap();
    let expected = fixtures["cellRange"]["expected"].as_array().unwrap();
    assert_eq!(cases.len(), expected.len());
    for (case, expected) in cases.iter().zip(expected) {
        let line = case[0].as_str().unwrap();
        let column = case[1].as_u64().unwrap() as usize;
        let actual = get_grapheme_cell_range(line, column);
        let expected = if expected.is_null() {
            None
        } else {
            let expected = expected.as_array().unwrap();
            Some((
                expected[0].as_u64().unwrap() as usize,
                expected[1].as_u64().unwrap() as usize,
            ))
        };
        assert_eq!(
            actual, expected,
            "cellRange mismatch for {line:?} at {column}"
        );
    }
}

#[test]
fn differential_get_osc8_link_at_column_matches_upstream() {
    let fixtures = fixtures();
    let cases = fixtures["osc8"]["cases"].as_array().unwrap();
    let expected = fixtures["osc8"]["expected"].as_array().unwrap();
    assert_eq!(cases.len(), expected.len());
    for (case, expected) in cases.iter().zip(expected) {
        let line = case[0].as_str().unwrap();
        let column = case[1].as_u64().unwrap() as usize;
        let actual = get_osc8_link_at_column(line, column);
        let expected = if expected.is_null() {
            None
        } else {
            Some(expected.as_str().unwrap())
        };
        assert_eq!(actual, expected, "osc8 mismatch for {line:?} at {column}");
    }
}

#[test]
fn differential_normalize_matches_upstream_byte_for_byte() {
    let fixtures = fixtures();
    let cases = fixtures["normalize"]["cases"].as_array().unwrap();
    let expected = fixtures["normalize"]["expected"].as_array().unwrap();
    assert_eq!(cases.len(), expected.len());
    for (input, expected) in cases.iter().zip(expected) {
        assert_eq!(
            normalize_terminal_output(input.as_str().unwrap()),
            expected.as_str().unwrap(),
            "normalize mismatch for {input:?}"
        );
    }
}

#[test]
fn differential_strip_matches_upstream_byte_for_byte() {
    let fixtures = fixtures();
    let cases = fixtures["strip"]["cases"].as_array().unwrap();
    let expected = fixtures["strip"]["expected"].as_array().unwrap();
    assert_eq!(cases.len(), expected.len());
    for (input, expected) in cases.iter().zip(expected) {
        assert_eq!(
            strip_terminal_sequences(input.as_str().unwrap()),
            expected.as_str().unwrap(),
            "strip mismatch for {input:?}"
        );
    }
}

#[test]
fn differential_active_background_matches_upstream() {
    let fixtures = fixtures();
    let cases = fixtures["activeBg"]["cases"].as_array().unwrap();
    let expected = fixtures["activeBg"]["expected"].as_array().unwrap();
    assert_eq!(cases.len(), expected.len());
    for (input, expected) in cases.iter().zip(expected) {
        assert_eq!(
            get_active_background_ansi(input.as_str().unwrap()),
            expected.as_str().unwrap(),
            "activeBg mismatch for {input:?}"
        );
    }
}

#[test]
fn rgi_emoji_table_has_known_members_and_non_members() {
    // Flag pair (RGI_Emoji_Flag_Sequence), family ZWJ sequence, and non-emoji.
    assert!(is_rgi_emoji_probe("\u{1f1e8}\u{1f1f3}"));
    assert!(is_rgi_emoji_probe(
        "\u{1f469}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}"
    ));
    assert!(!is_rgi_emoji_probe("a"));
}
