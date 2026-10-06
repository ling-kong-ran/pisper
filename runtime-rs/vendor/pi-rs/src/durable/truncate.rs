//! Port of `src/truncate.ts`: shared truncation utilities for tool results.
//!
//! Truncation is based on two independent limits — whichever is hit first
//! wins: line limit (default 2000) and byte limit (default 50 KiB). Never
//! returns partial lines. Tool output streams are bounded by
//! [`super::harness::output`] instead.

/// `DEFAULT_MAX_LINES` (`truncate.ts:11`).
pub const DEFAULT_MAX_LINES: usize = 2000;
/// `DEFAULT_MAX_BYTES` (`truncate.ts:12`): 50 KiB.
pub const DEFAULT_MAX_BYTES: usize = 50 * 1024;

/// Which limit was hit (`truncate.ts` `truncatedBy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TruncatedBy {
    Lines,
    Bytes,
}

/// `TruncationResult` (`truncate.ts:14-37`). Field order follows the
/// upstream literal (`content`, `truncated`, `truncatedBy`, `totalLines`,
/// `totalBytes`, `outputLines`, `outputBytes`, `lastLinePartial`,
/// `firstLineExceedsLimit`, `maxLines`, `maxBytes`).
#[derive(Debug, Clone, PartialEq)]
pub struct TruncationResult {
    /// The truncated content.
    pub content: String,
    /// Whether truncation occurred.
    pub truncated: bool,
    /// Which limit was hit, `None` when not truncated.
    pub truncated_by: Option<TruncatedBy>,
    /// Total number of lines in the original content.
    pub total_lines: usize,
    /// Total number of bytes in the original content.
    pub total_bytes: usize,
    /// Number of complete lines in the truncated output.
    pub output_lines: usize,
    /// Number of bytes in the truncated output.
    pub output_bytes: usize,
    /// Whether the last line was partially truncated (tail edge case).
    pub last_line_partial: bool,
    /// Whether the first line exceeded the byte limit (head truncation).
    pub first_line_exceeds_limit: bool,
    /// The max lines limit that was applied.
    pub max_lines: usize,
    /// The max bytes limit that was applied.
    pub max_bytes: usize,
}

/// `TruncationOptions` (`truncate.ts:39-44`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TruncationOptions {
    /// Maximum number of lines (default 2000).
    pub max_lines: Option<usize>,
    /// Maximum number of bytes (default 50 KiB).
    pub max_bytes: Option<usize>,
}

/// UTF-8 byte length (`truncate.ts` `utf8ByteLength`): the port counts the
/// encoded bytes of the string directly, which equals the upstream
/// `Buffer.byteLength` fast path.
pub fn utf8_byte_length(content: &str) -> usize {
    content.len()
}

/// `splitLinesForCounting` (`truncate.ts:81-86`): split on `\n`, dropping a
/// trailing empty piece when the content ends with a newline.
fn split_lines_for_counting(content: &str) -> Vec<&str> {
    if content.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = content.split('\n').collect();
    if content.ends_with('\n') {
        lines.pop();
    }
    lines
}

/// Format bytes as human-readable size (`truncate.ts` `formatSize`).
pub fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Truncate content from the head (`truncate.ts` `truncateHead`): keep the
/// first N lines/bytes. Suitable for file reads where you want to see the
/// beginning. Never returns partial lines. If the first line exceeds the
/// byte limit, returns empty content with `firstLineExceedsLimit = true`.
pub fn truncate_head(content: &str, options: TruncationOptions) -> TruncationResult {
    let max_lines = options.max_lines.unwrap_or(DEFAULT_MAX_LINES);
    let max_bytes = options.max_bytes.unwrap_or(DEFAULT_MAX_BYTES);

    let total_bytes = utf8_byte_length(content);
    let lines = split_lines_for_counting(content);
    let total_lines = lines.len();

    // Check if no truncation is needed.
    if total_lines <= max_lines && total_bytes <= max_bytes {
        return TruncationResult {
            content: content.to_string(),
            truncated: false,
            truncated_by: None,
            total_lines,
            total_bytes,
            output_lines: total_lines,
            output_bytes: total_bytes,
            last_line_partial: false,
            first_line_exceeds_limit: false,
            max_lines,
            max_bytes,
        };
    }

    // Check if the first line alone exceeds the byte limit.
    let first_line_bytes = utf8_byte_length(lines[0]);
    if first_line_bytes > max_bytes {
        return TruncationResult {
            content: String::new(),
            truncated: true,
            truncated_by: Some(TruncatedBy::Bytes),
            total_lines,
            total_bytes,
            output_lines: 0,
            output_bytes: 0,
            last_line_partial: false,
            first_line_exceeds_limit: true,
            max_lines,
            max_bytes,
        };
    }

    // Collect complete lines that fit.
    let mut output_lines_arr: Vec<&str> = Vec::new();
    let mut output_bytes_count = 0usize;
    let mut truncated_by = TruncatedBy::Lines;

    for (index, line) in lines.iter().take(max_lines).enumerate() {
        // +1 for the newline separator after the first line.
        let line_bytes = utf8_byte_length(line) + usize::from(index > 0);
        if output_bytes_count + line_bytes > max_bytes {
            truncated_by = TruncatedBy::Bytes;
            break;
        }
        output_lines_arr.push(line);
        output_bytes_count += line_bytes;
    }

    // Without a byte break, only omitted lines prove the line limit was
    // reached; otherwise a trailing newline exceeded bytes.
    let truncated_by = if truncated_by == TruncatedBy::Bytes {
        truncated_by
    } else if output_lines_arr.len() < total_lines {
        TruncatedBy::Lines
    } else {
        TruncatedBy::Bytes
    };

    let output_content = output_lines_arr.join("\n");
    let final_output_bytes = utf8_byte_length(&output_content);

    TruncationResult {
        content: output_content,
        truncated: true,
        truncated_by: Some(truncated_by),
        total_lines,
        total_bytes,
        output_lines: output_lines_arr.len(),
        output_bytes: final_output_bytes,
        last_line_partial: false,
        first_line_exceeds_limit: false,
        max_lines,
        max_bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_length_matches_encoding() {
        assert_eq!(utf8_byte_length("hello"), 5);
        assert_eq!(utf8_byte_length("héllo"), 6);
        assert_eq!(utf8_byte_length("日本語"), 9);
        assert_eq!(utf8_byte_length(""), 0);
    }

    #[test]
    fn head_truncation_by_lines() {
        let content = "a\nb\nc\nd\n";
        let result = truncate_head(
            content,
            TruncationOptions {
                max_lines: Some(2),
                max_bytes: Some(100),
            },
        );
        assert_eq!(result.content, "a\nb");
        assert_eq!(result.truncated_by, Some(TruncatedBy::Lines));
        assert_eq!(result.total_lines, 4);
        assert_eq!(result.output_lines, 2);
    }

    #[test]
    fn head_truncation_by_bytes_never_splits_lines() {
        let content = "abc\ndef\nghi";
        let result = truncate_head(
            content,
            TruncationOptions {
                max_lines: Some(100),
                max_bytes: Some(7),
            },
        );
        // "abc\ndef" is exactly 7 bytes; "ghi" would exceed the limit.
        assert_eq!(result.content, "abc\ndef");
        assert_eq!(result.truncated_by, Some(TruncatedBy::Bytes));
    }

    #[test]
    fn first_line_over_limit_is_reported() {
        let result = truncate_head(
            "abcdef\nx",
            TruncationOptions {
                max_lines: Some(10),
                max_bytes: Some(4),
            },
        );
        assert!(result.first_line_exceeds_limit);
        assert_eq!(result.content, "");
        assert_eq!(result.truncated_by, Some(TruncatedBy::Bytes));
    }
}
