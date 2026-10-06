//! Port of `packages/agent/src/harness/utils/truncate.ts:1-336` (the subset
//! the execution-environment output capture needs): byte/line truncation for
//! bounded shell-output views.
//!
//! Ported here (M3b Task 6) because [`OutputCapture`](super::output_capture::OutputCapture)
//! and `executeShellWithCapture` are the only harness consumers in this task's
//! scope; the grep-specific helpers (`GREP_MAX_LINE_LENGTH`, `truncateLine`)
//! and `formatSize` land with the tools that use them (M3b Task 11).
//!
//! Disclosed substitutions:
//! - Upstream operates on JS strings (UTF-16 code units) while counting bytes
//!   with `utf8ByteLength`. Every algorithm here is line- and byte-based, and
//!   lines are split on `\n`, which never falls inside a multi-byte sequence,
//!   so `&str` iteration (`chars`/bytes) is behavior-identical for all valid
//!   inputs. The one upstream function that slices within a line
//!   (`truncateStringToBytesFromEnd`) walks surrogate pairs explicitly
//!   (`truncate.ts:301-336`); the port walks `char`s, which is the same walk
//!   without the surrogate bookkeeping (Rust strings cannot hold unpaired
//!   surrogates, so the `needsReplacement` branch is unreachable).
//! - `TruncationResult` keeps the full upstream shape including `content`
//!   (`truncate.ts:15-38`); the content-free metadata view is
//!   [`crate::agent_core::harness::types::ShellOutputTruncation`] (M3b Task
//!   2) and is produced by [`TruncationResult::truncation_metadata`].

use serde::{Deserialize, Serialize};

use crate::agent_core::harness::types::{ShellOutputTruncation, TruncatedBy};

/// Upstream `DEFAULT_MAX_LINES` (`truncate.ts:11`).
pub const DEFAULT_MAX_LINES: u64 = 2000;

/// Upstream `DEFAULT_MAX_BYTES` (`truncate.ts:12`): 50KB.
pub const DEFAULT_MAX_BYTES: u64 = 50 * 1024;

/// Upstream `TruncationResult` (`truncate.ts:15-38`): the truncated content
/// plus the full truncation metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TruncationResult {
    /// The truncated content.
    pub content: String,
    /// Whether truncation occurred.
    pub truncated: bool,
    /// Which limit was hit; `None` when not truncated (`truncatedBy: null`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated_by: Option<TruncatedBy>,
    /// Total number of lines in the original content.
    pub total_lines: u64,
    /// Total number of bytes in the original content.
    pub total_bytes: u64,
    /// Number of complete lines in the truncated output.
    pub output_lines: u64,
    /// Number of bytes in the truncated output.
    pub output_bytes: u64,
    /// Whether the last line was partially truncated (tail-truncation edge).
    pub last_line_partial: bool,
    /// Whether the first line exceeded the byte limit (head truncation).
    pub first_line_exceeds_limit: bool,
    /// The max lines limit that was applied.
    pub max_lines: u64,
    /// The max bytes limit that was applied.
    pub max_bytes: u64,
}

impl TruncationResult {
    /// The content-free metadata view (upstream `Omit<TruncationResult,
    /// "content">` spread in `output-capture.ts:100`).
    pub fn truncation_metadata(&self) -> ShellOutputTruncation {
        ShellOutputTruncation {
            truncated: self.truncated,
            truncated_by: self.truncated_by,
            total_lines: self.total_lines,
            total_bytes: self.total_bytes,
            output_lines: self.output_lines,
            output_bytes: self.output_bytes,
            last_line_partial: self.last_line_partial,
            first_line_exceeds_limit: self.first_line_exceeds_limit,
            max_lines: self.max_lines,
            max_bytes: self.max_bytes,
        }
    }

    /// Rebuild a [`TruncationResult`] from the content-free metadata view and
    /// the content (the reverse of [`Self::truncation_metadata`]; used by the
    /// `shell-output.ts` port where upstream spreads `{ content,
    /// ...truncation }` back together, `shell-output.ts:37`).
    pub fn from_metadata(content: &str, metadata: &ShellOutputTruncation) -> Self {
        TruncationResult {
            content: content.to_string(),
            truncated: metadata.truncated,
            truncated_by: metadata.truncated_by,
            total_lines: metadata.total_lines,
            total_bytes: metadata.total_bytes,
            output_lines: metadata.output_lines,
            output_bytes: metadata.output_bytes,
            last_line_partial: metadata.last_line_partial,
            first_line_exceeds_limit: metadata.first_line_exceeds_limit,
            max_lines: metadata.max_lines,
            max_bytes: metadata.max_bytes,
        }
    }
}

/// Upstream `TruncationOptions` (`truncate.ts:40-45`); `None` fields use the
/// defaults ([`DEFAULT_MAX_LINES`] / [`DEFAULT_MAX_BYTES`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TruncationOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_lines: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
}

impl TruncationOptions {
    fn max_lines(&self) -> u64 {
        self.max_lines.unwrap_or(DEFAULT_MAX_LINES)
    }

    fn max_bytes(&self) -> u64 {
        self.max_bytes.unwrap_or(DEFAULT_MAX_BYTES)
    }
}

/// Upstream `utf8ByteLength` (`truncate.ts:54-80`): the UTF-8 encoded byte
/// length. Rust strings are UTF-8, so this is `str::len`.
pub fn utf8_byte_length(content: &str) -> usize {
    content.len()
}

/// Upstream `splitLinesForCounting` (`truncate.ts:82-87`): split on `\n`,
/// dropping the trailing empty segment produced by a final newline.
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

fn no_truncation(
    content: &str,
    total_lines: u64,
    total_bytes: u64,
    options: &TruncationOptions,
) -> TruncationResult {
    TruncationResult {
        content: content.to_string(),
        truncated: false,
        truncated_by: None,
        total_lines,
        total_bytes,
        output_lines: total_lines,
        output_bytes: total_bytes,
        last_line_partial: false,
        first_line_exceeds_limit: false,
        max_lines: options.max_lines(),
        max_bytes: options.max_bytes(),
    }
}

#[allow(clippy::too_many_arguments)] // mirrors the upstream result literal's fields
fn truncated_result(
    content: String,
    truncated_by: Option<TruncatedBy>,
    total_lines: u64,
    total_bytes: u64,
    output_lines: u64,
    last_line_partial: bool,
    first_line_exceeds_limit: bool,
    options: &TruncationOptions,
) -> TruncationResult {
    let output_bytes = utf8_byte_length(&content) as u64;
    TruncationResult {
        content,
        truncated: true,
        truncated_by,
        total_lines,
        total_bytes,
        output_lines,
        output_bytes,
        last_line_partial,
        first_line_exceeds_limit,
        max_lines: options.max_lines(),
        max_bytes: options.max_bytes(),
    }
}

/// Upstream `truncateHead` (`truncate.ts:132-214`): keep the first
/// `maxLines`/`maxBytes`. Never returns partial lines; when the first line
/// alone exceeds the byte limit the content is empty with
/// `firstLineExceedsLimit: true`.
pub fn truncate_head(content: &str, options: TruncationOptions) -> TruncationResult {
    let max_lines = options.max_lines();
    let max_bytes = options.max_bytes();

    let total_bytes = utf8_byte_length(content) as u64;
    let lines = split_lines_for_counting(content);
    let total_lines = lines.len() as u64;

    if total_lines <= max_lines && total_bytes <= max_bytes {
        return no_truncation(content, total_lines, total_bytes, &options);
    }

    // First line alone exceeds the byte limit.
    let first_line_bytes = utf8_byte_length(lines[0]) as u64;
    if first_line_bytes > max_bytes {
        return truncated_result(
            String::new(),
            Some(TruncatedBy::Bytes),
            total_lines,
            total_bytes,
            0,
            false,
            true,
            &options,
        );
    }

    let mut output_lines: Vec<&str> = Vec::new();
    let mut output_bytes_count: u64 = 0;
    let mut truncated_by = TruncatedBy::Lines;

    for (index, line) in lines.iter().enumerate() {
        if index as u64 >= max_lines {
            break;
        }
        // +1 for the newline (truncate.ts:182).
        let line_bytes = utf8_byte_length(line) as u64 + u64::from(index > 0);
        if output_bytes_count + line_bytes > max_bytes {
            truncated_by = TruncatedBy::Bytes;
            break;
        }
        output_lines.push(line);
        output_bytes_count += line_bytes;
    }

    // Exited due to the line limit (truncate.ts:194-196).
    if output_lines.len() as u64 >= max_lines && output_bytes_count <= max_bytes {
        truncated_by = TruncatedBy::Lines;
    }

    truncated_result(
        output_lines.join("\n"),
        Some(truncated_by),
        total_lines,
        total_bytes,
        output_lines.len() as u64,
        false,
        false,
        &options,
    )
}

/// Upstream `truncateTail` (`truncate.ts:222-295`): keep the last
/// `maxLines`/`maxBytes`. May return a partial first line when the last line
/// of the original content exceeds the byte limit.
pub fn truncate_tail(content: &str, options: TruncationOptions) -> TruncationResult {
    let max_lines = options.max_lines();
    let max_bytes = options.max_bytes();

    let total_bytes = utf8_byte_length(content) as u64;
    let lines = split_lines_for_counting(content);
    let total_lines = lines.len() as u64;

    if total_lines <= max_lines && total_bytes <= max_bytes {
        return no_truncation(content, total_lines, total_bytes, &options);
    }

    let mut output_lines: Vec<String> = Vec::new();
    let mut output_bytes_count: u64 = 0;
    let mut truncated_by = TruncatedBy::Lines;
    let mut last_line_partial = false;

    for line in lines.iter().rev() {
        if output_lines.len() as u64 >= max_lines {
            break;
        }
        // +1 for the newline (truncate.ts:255).
        let line_bytes = utf8_byte_length(line) as u64 + u64::from(!output_lines.is_empty());
        if output_bytes_count + line_bytes > max_bytes {
            truncated_by = TruncatedBy::Bytes;
            // Edge case: no lines added yet and this line exceeds maxBytes —
            // keep the end of the line, partially (truncate.ts:259-266).
            if output_lines.is_empty() {
                let truncated_line = truncate_string_to_bytes_from_end(line, max_bytes);
                output_bytes_count = utf8_byte_length(&truncated_line) as u64;
                output_lines.insert(0, truncated_line);
                last_line_partial = true;
            }
            break;
        }
        output_lines.insert(0, line.to_string());
        output_bytes_count += line_bytes;
    }

    // Exited due to the line limit (truncate.ts:275-277).
    if output_lines.len() as u64 >= max_lines && output_bytes_count <= max_bytes {
        truncated_by = TruncatedBy::Lines;
    }

    truncated_result(
        output_lines.join("\n"),
        Some(truncated_by),
        total_lines,
        total_bytes,
        output_lines.len() as u64,
        last_line_partial,
        false,
        &options,
    )
}

/// Upstream `truncateStringToBytesFromEnd` (`truncate.ts:301-336`): keep at
/// most `max_bytes` bytes counting from the end of the string, never splitting
/// a character.
fn truncate_string_to_bytes_from_end(text: &str, max_bytes: u64) -> String {
    if max_bytes == 0 {
        return String::new();
    }
    let mut output_bytes: u64 = 0;
    let mut start = text.len();
    for (index, character) in text.char_indices().rev() {
        let character_bytes = character.len_utf8() as u64;
        if output_bytes + character_bytes > max_bytes {
            break;
        }
        output_bytes += character_bytes;
        start = index;
    }
    text[start..].to_string()
}

/// Upstream truncate.ts formatSize: display bytes with one fractional digit.
pub fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        {
            let tenths = (bytes as f64 / 1024.0 * 10.0).round() as u64;
            format!("{}.{:01}KB", tenths / 10, tenths % 10)
        }
    } else {
        {
            let tenths = (bytes as f64 / (1024.0 * 1024.0) * 10.0).round() as u64;
            format!("{}.{:01}MB", tenths / 10, tenths % 10)
        }
    }
}

#[cfg(test)]
mod tests;
