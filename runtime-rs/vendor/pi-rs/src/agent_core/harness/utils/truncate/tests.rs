//! Sanity checks for the truncate subset (the full
//! `test/harness/truncate.test.ts` oracle lands with M3b Task 11); these pin
//! the behaviors the output capture relies on.

use super::*;

#[test]
fn under_the_limits_returns_content_unchanged() {
    let result = truncate_tail("one\ntwo\n", TruncationOptions::default());
    assert_eq!(result.content, "one\ntwo\n");
    assert!(!result.truncated);
    assert_eq!(result.truncated_by, None);
    assert_eq!(result.total_lines, 2);
    assert_eq!(result.total_bytes, 8);
    assert_eq!(result.max_lines, DEFAULT_MAX_LINES);
    assert_eq!(result.max_bytes, DEFAULT_MAX_BYTES);
}

#[test]
fn head_truncation_keeps_leading_lines_and_reports_bytes() {
    let result = truncate_head(
        "one\ntwo\nthree",
        TruncationOptions {
            max_lines: Some(10),
            max_bytes: Some(8),
        },
    );
    assert_eq!(result.content, "one\ntwo");
    assert!(result.truncated);
    assert_eq!(result.truncated_by, Some(TruncatedBy::Bytes));
    assert_eq!(result.total_lines, 3);
    assert_eq!(result.output_lines, 2);
    assert!(!result.last_line_partial);
}

#[test]
fn tail_truncation_keeps_trailing_lines_and_reports_lines() {
    let result = truncate_tail(
        "one\ntwo\nthree",
        TruncationOptions {
            max_lines: Some(2),
            max_bytes: Some(1024),
        },
    );
    assert_eq!(result.content, "two\nthree");
    assert!(result.truncated);
    assert_eq!(result.truncated_by, Some(TruncatedBy::Lines));
}

#[test]
fn tail_edge_case_keeps_a_partial_last_line() {
    // The single line exceeds maxBytes; the tail edge keeps its end.
    let result = truncate_tail(
        "abcdefgh",
        TruncationOptions {
            max_lines: Some(10),
            max_bytes: Some(4),
        },
    );
    assert_eq!(result.content, "efgh");
    assert!(result.last_line_partial);
    assert_eq!(result.truncated_by, Some(TruncatedBy::Bytes));
    assert_eq!(result.output_bytes, 4);
}

#[test]
fn first_line_exceeding_the_byte_limit_empties_head_output() {
    let result = truncate_head(
        "abcdefgh\nsecond",
        TruncationOptions {
            max_lines: Some(10),
            max_bytes: Some(4),
        },
    );
    assert_eq!(result.content, "");
    assert!(result.first_line_exceeds_limit);
    assert_eq!(result.truncated_by, Some(TruncatedBy::Bytes));
}

#[test]
fn metadata_view_carries_every_non_content_field() {
    let result = truncate_tail("x", TruncationOptions::default());
    let metadata = result.truncation_metadata();
    assert!(!metadata.truncated);
    assert_eq!(metadata.total_bytes, 1);
    assert_eq!(metadata.max_bytes, DEFAULT_MAX_BYTES);
    assert!(!metadata.last_line_partial);
}

#[test]
fn multi_byte_content_counts_utf8_bytes() {
    // "héllo" is 6 UTF-8 bytes (truncate.ts:54-80).
    assert_eq!(utf8_byte_length("héllo"), 6);
    let head = truncate_head(
        "héllo\nsecond",
        TruncationOptions {
            max_lines: Some(10),
            max_bytes: Some(6),
        },
    );
    assert_eq!(head.content, "héllo");
    assert_eq!(head.output_bytes, 6);
    let tail = truncate_tail(
        "héllo\nsecond",
        TruncationOptions {
            max_lines: Some(10),
            max_bytes: Some(6),
        },
    );
    assert_eq!(tail.content, "second");
    assert_eq!(tail.output_bytes, 6);
}
