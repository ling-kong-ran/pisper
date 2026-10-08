//! Port of `src/tools/read.ts`: reads text files. Remarks about truncation
//! and continuation are diagnostics; the content is only file text.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;

use crate::agent_core::chord_support::context::Context;
use crate::ai::types::{TextContent, TextOrImageBlock, Tool as AiTool};
use crate::durable::errors::PlainError;
use crate::durable::harness::output::character_end;
use crate::durable::harness::types::{
    ToolDiagnostic, ToolDiagnosticSeverity, ToolExecutionApiLike, ToolExecutionResult,
    ToolRegistration,
};
use crate::durable::tools::env::require_env;
use crate::durable::tools::image::detect_supported_image_mime_type;
use crate::durable::tools::path_utils::resolve_read_tool_path;
use crate::durable::truncate::{
    format_size, truncate_head, TruncatedBy, TruncationOptions, TruncationResult,
    DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES,
};

/// `ReadToolInput` (`tools/read.ts`); validated upstream by the tool
/// declaration before `execute` runs (D32).
#[derive(Debug, Clone, Deserialize)]
pub struct ReadToolInput {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
}

/// `ReadToolDetails` (`tools/read.ts`): how the shown text was cut; the text
/// itself is the result content. Wire order follows `TruncationResult` less
/// `content`.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadToolDetails {
    pub truncation: Option<TruncationDetails>,
}

/// `Omit<TruncationResult, "content">` wire form.
#[derive(Debug, Clone, PartialEq)]
pub struct TruncationDetails {
    pub truncated: bool,
    pub truncated_by: Option<TruncatedBy>,
    pub total_lines: usize,
    pub total_bytes: usize,
    pub output_lines: usize,
    pub output_bytes: usize,
    pub last_line_partial: bool,
    pub first_line_exceeds_limit: bool,
    pub max_lines: usize,
    pub max_bytes: usize,
}

impl TruncationDetails {
    fn of(result: &TruncationResult) -> Self {
        TruncationDetails {
            truncated: result.truncated,
            truncated_by: result.truncated_by,
            total_lines: result.total_lines,
            total_bytes: result.total_bytes,
            output_lines: result.output_lines,
            output_bytes: result.output_bytes,
            last_line_partial: result.last_line_partial,
            first_line_exceeds_limit: result.first_line_exceeds_limit,
            max_lines: result.max_lines,
            max_bytes: result.max_bytes,
        }
    }

    /// The JSON wire form; key order is the upstream literal's.
    pub fn to_json(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        map.insert(String::from("truncated"), json!(self.truncated));
        map.insert(
            String::from("truncatedBy"),
            match self.truncated_by {
                Some(TruncatedBy::Lines) => json!("lines"),
                Some(TruncatedBy::Bytes) => json!("bytes"),
                None => serde_json::Value::Null,
            },
        );
        map.insert(String::from("totalLines"), json!(self.total_lines));
        map.insert(String::from("totalBytes"), json!(self.total_bytes));
        map.insert(String::from("outputLines"), json!(self.output_lines));
        map.insert(String::from("outputBytes"), json!(self.output_bytes));
        map.insert(
            String::from("lastLinePartial"),
            json!(self.last_line_partial),
        );
        map.insert(
            String::from("firstLineExceedsLimit"),
            json!(self.first_line_exceeds_limit),
        );
        map.insert(String::from("maxLines"), json!(self.max_lines));
        map.insert(String::from("maxBytes"), json!(self.max_bytes));
        serde_json::Value::Object(map)
    }
}

/// `createReadTool()` (`tools/read.ts`).
pub fn create_read_tool() -> ToolRegistration {
    ToolRegistration {
        tool: AiTool {
            name: String::from("read"),
            description: format!(
                "Read the contents of a text file. Output is truncated to {DEFAULT_MAX_LINES} lines or {}KB (whichever is hit first). Use offset/limit for large files. When you need the full file, continue with offset until complete.",
                DEFAULT_MAX_BYTES / 1024
            ),
            parameters: json!({
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": {"type": "string", "description": "Path to the file to read (relative or absolute)"},
                    "offset": {"type": "number", "description": "Line number to start reading from (1-indexed)"},
                    "limit": {"type": "number", "description": "Maximum number of lines to read"},
                },
            }),
            constrained_sampling: None,
        },
        replay: None,
        execution_mode: None,
        prepare_arguments: None,
        output_limits: None,
        execute: Arc::new(
            |args: serde_json::Map<String, serde_json::Value>,
             api: Arc<dyn ToolExecutionApiLike>,
             context: Context| {
                Box::pin(async move {
                    let input: ReadToolInput =
                        serde_json::from_value(serde_json::Value::Object(args))
                            .map_err(|error| PlainError::new(error.to_string()))?;
                    let env = require_env(api.as_ref())?;
                    let absolute_path =
                        resolve_read_tool_path(env.as_ref(), &input.path, &context)?;
                    let bytes = env
                        .read_binary_file(&absolute_path, &context)
                        .map_err(|error| PlainError::new(error.message))?;
                    if let Some(mime_type) = detect_supported_image_mime_type(&bytes) {
                        // Image content is not supported yet.
                        return Ok(ToolExecutionResult {
                            content: Some(Vec::new()),
                            is_error: Some(true),
                            details: None,
                            diagnostics: Some(vec![ToolDiagnostic {
                                severity: ToolDiagnosticSeverity::Error,
                                code: Some(String::from("unsupported_image")),
                                message: format!(
                                    "{} is an image ({mime_type}); reading images is not supported",
                                    input.path
                                ),
                            }]),
                            usage: None,
                            control: None,
                        });
                    }

                    // `new TextDecoder().decode(bytes)`: invalid UTF-8 becomes
                    // replacement characters.
                    let text_content = String::from_utf8_lossy(&bytes);
                    let text_content = text_content.as_ref();
                    let all_lines: Vec<&str> = text_content.split('\n').collect();
                    let total_file_lines = all_lines.len();
                    let start_line = match input.offset {
                        Some(offset) if offset != 0.0 => (offset - 1.0).max(0.0),
                        _ => 0.0,
                    };
                    let start_index = slice_index(start_line, all_lines.len());
                    let start_line_display = start_index + 1;
                    if start_line >= all_lines.len() as f64 {
                        return Err(PlainError::new(format!(
                            "Offset {} is beyond end of file ({} lines total)",
                            input.offset.unwrap_or_default(),
                            all_lines.len()
                        )));
                    }

                    let mut user_limited_lines: Option<f64> = None;
                    let selected_content = match input.limit {
                        Some(limit) => {
                            let end_line = (start_line + limit).min(all_lines.len() as f64);
                            let end_index = slice_index(end_line, all_lines.len());
                            user_limited_lines = Some(end_line - start_line);
                            all_lines[start_index..end_index.max(start_index)].join("\n")
                        }
                        None => all_lines[start_index..].join("\n"),
                    };

                    let truncation = truncate_head(&selected_content, TruncationOptions::default());
                    let mut diagnostics: Vec<ToolDiagnostic> = Vec::new();
                    let mut output_text = truncation.content.clone();
                    let mut details: Option<serde_json::Value> = None;
                    if truncation.first_line_exceeds_limit {
                        // Show the start of the line, cut at the byte limit on
                        // a character boundary.
                        let line_bytes = all_lines[start_index].as_bytes();
                        let end = character_end(line_bytes, DEFAULT_MAX_BYTES);
                        output_text =
                            String::from_utf8_lossy(&line_bytes[..end]).into_owned();
                        diagnostics.push(ToolDiagnostic {
                            severity: ToolDiagnosticSeverity::Warn,
                            code: Some(String::from("truncated")),
                            message: format!(
                                "Line {start_line_display} is {}, exceeds the {} limit; showing its first {}. Use bash: sed -n '{start_line_display}p' {} | tail -c +{}",
                                format_size(line_bytes.len()),
                                format_size(DEFAULT_MAX_BYTES),
                                format_size(end),
                                input.path,
                                end + 1
                            ),
                        });
                        let mut detail = TruncationDetails::of(&truncation);
                        detail.output_bytes = end;
                        detail.output_lines = 1;
                        details = Some(json!({ "truncation": detail.to_json() }));
                    } else if truncation.truncated {
                        let end_line_display = start_line_display + truncation.output_lines - 1;
                        let next_offset = end_line_display + 1;
                        let limit_text = if truncation.truncated_by == Some(TruncatedBy::Lines) {
                            String::new()
                        } else {
                            format!(" ({} limit)", format_size(DEFAULT_MAX_BYTES))
                        };
                        diagnostics.push(ToolDiagnostic {
                            severity: ToolDiagnosticSeverity::Info,
                            code: Some(String::from("truncated")),
                            message: format!(
                                "Showing lines {start_line_display}-{end_line_display} of {total_file_lines}{limit_text}. Use offset={next_offset} to continue."
                            ),
                        });
                        details = Some(json!({ "truncation": TruncationDetails::of(&truncation).to_json() }));
                    } else if let Some(user_limited_lines) = user_limited_lines {
                        if start_line + user_limited_lines < all_lines.len() as f64 {
                            let remaining = all_lines.len() as f64 - (start_line + user_limited_lines);
                            let next_offset = start_line + user_limited_lines + 1.0;
                            diagnostics.push(ToolDiagnostic {
                                severity: ToolDiagnosticSeverity::Info,
                                code: None,
                                message: format!(
                                    "{remaining} more lines in file. Use offset={next_offset} to continue."
                                ),
                            });
                        }
                    }

                    let content = if output_text.is_empty() {
                        Vec::new()
                    } else {
                        vec![TextOrImageBlock::Text(TextContent {
                            text: output_text,
                            text_signature: None,
                        })]
                    };
                    Ok(ToolExecutionResult {
                        content: Some(content),
                        is_error: None,
                        details,
                        // The upstream literal always carries `diagnostics`.
                        diagnostics: Some(diagnostics),
                        usage: None,
                        control: None,
                    })
                })
            },
        ),
    }
}

/// JS `Array.prototype.slice` index coercion (ToIntegerOrInfinity, then
/// negative resolution), shared by the offset/limit math.
fn slice_index(value: f64, length: usize) -> usize {
    let value = value.trunc();
    if value < 0.0 {
        (length as f64 + value).max(0.0) as usize
    } else {
        (value as usize).min(length)
    }
}
