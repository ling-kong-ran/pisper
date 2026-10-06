//! Upstream tools/read.ts: bounded text and content-sniffed image attachments.
use super::super::utils::truncate::{
    format_size, truncate_head, TruncationOptions, TruncationResult, DEFAULT_MAX_BYTES,
    DEFAULT_MAX_LINES,
};
use super::super::{AgentHarnessTool, Context, TruncatedBy};
use super::image::{detect_supported_image_mime_type, encode_base64};
use super::path_utils::resolve_read_tool_path;
use super::{text_result, ExecutionToolContext, HasExecutionEnv};
use crate::ai::types::{ImageContent, TextOrImageBlock};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadToolInput {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadToolDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncation: Option<TruncationResult>,
}
#[derive(Debug, Clone)]
pub enum ReadImageProcessorResult {
    Success {
        data: String,
        mime_type: String,
        hints: Vec<String>,
    },
    Failure {
        message: String,
    },
}
#[derive(Debug, Clone, Copy)]
pub struct ReadImageProcessorOptions {
    pub auto_resize_images: bool,
}
pub type ReadImageProcessor = dyn Fn(
        Vec<u8>,
        String,
        ReadImageProcessorOptions,
        Context,
    ) -> BoxFuture<'static, anyhow::Result<ReadImageProcessorResult>>
    + Send
    + Sync;
#[derive(Clone, Default)]
pub struct ReadToolOptions {
    pub auto_resize_images: Option<bool>,
    pub image_processor: Option<Arc<ReadImageProcessor>>,
}

pub fn create_read_tool(options: ReadToolOptions) -> AgentHarnessTool<ExecutionToolContext> {
    create_read_tool_for(options)
}
pub fn create_read_tool_for<T: HasExecutionEnv>(options: ReadToolOptions) -> AgentHarnessTool<T> {
    AgentHarnessTool {
        name: "read".into(), label: "read".into(),
        description: format!("Read the contents of a file. Supports text files and images (jpg, png, gif, webp, bmp). Images are sent as attachments. For text files, output is truncated to {DEFAULT_MAX_LINES} lines or {}KB (whichever is hit first). Use offset/limit for large files. When you need the full file, continue with offset until complete.", DEFAULT_MAX_BYTES/1024),
        parameters: json!({"type":"object","properties":{"path":{"type":"string","description":"Path to the file to read (relative or absolute)"},"offset":{"type":"number","description":"Line number to start reading from (1-indexed)"},"limit":{"type":"number","description":"Maximum number of lines to read"}},"required":["path"]}),
        constrained_sampling: None, prepare_arguments: None, replay: None, execution_mode: None,
        execute: Arc::new(move |_, input, _, tool_context: T, _, context| {
            let options = options.clone();
            Box::pin(async move {
                let input: ReadToolInput = serde_json::from_value(input)?;
                let env = tool_context.execution_env();
                let absolute = resolve_read_tool_path(env.as_ref(), &input.path, context.clone()).await?;
                let bytes = env.read_binary_file(&absolute, context.clone()).await?;
                if let Some(mime) = detect_supported_image_mime_type(&bytes) {
                    if let Some(processor) = options.image_processor {
                        return match processor(bytes, mime.to_string(), ReadImageProcessorOptions { auto_resize_images: options.auto_resize_images.unwrap_or(true) }, context).await? {
                            ReadImageProcessorResult::Failure { message } => Ok(text_result(format!("Read image file [{mime}]\n{message}"))),
                            ReadImageProcessorResult::Success { data, mime_type, hints } => {
                                let hints = if hints.is_empty() { String::new() } else { format!("\n{}", hints.join("\n")) };
                                let mut result = text_result(format!("Read image file [{mime_type}]{hints}"));
                                result.content.push(TextOrImageBlock::Image(ImageContent { data, mime_type }));
                                Ok(result)
                            }
                        };
                    }
                    if mime == "image/bmp" { return Ok(text_result("Read image file [image/bmp]\n[Image omitted: configure an imageProcessor to convert BMP images.]")); }
                    let mut result = text_result(format!("Read image file [{mime}]"));
                    result.content.push(TextOrImageBlock::Image(ImageContent { data: encode_base64(&bytes), mime_type: mime.into() }));
                    return Ok(result);
                }
                // TextDecoder's default UTF-8 BOM handling consumes one BOM.
                let text = String::from_utf8_lossy(&bytes);
                let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
                let lines: Vec<_> = text.split('\n').collect();
                let start = input.offset.filter(|v| *v != 0.0).map(|v| (v-1.0).max(0.0)).unwrap_or(0.0);
                let display = start+1.0;
                if start >= lines.len() as f64 { anyhow::bail!("Offset {} is beyond end of file ({} lines total)", input.offset.unwrap_or(0.0), lines.len()); }
                let end = input.limit.map(|limit| (start+limit).min(lines.len() as f64));
                let start_index = slice_index(start, lines.len());
                let end_index = end.map(|v| slice_index(v,lines.len())).unwrap_or(lines.len()).max(start_index);
                let selected = lines[start_index..end_index].join("\n");
                let limited = end.map(|end| end-start);
                let truncation = truncate_head(&selected, TruncationOptions::default());
                let mut details = None;
                let output = if truncation.first_line_exceeds_limit {
                    let size = format_size(lines[start_index].len() as u64);
                    let output = format!("[Line {display} is {size}, exceeds {} limit. Use bash: sed -n '{display}p' {} | head -c {DEFAULT_MAX_BYTES}]", format_size(DEFAULT_MAX_BYTES), input.path);
                    details = Some(json!({"truncation":truncation})); output
                } else if truncation.truncated {
                    let end_display = display + truncation.output_lines as f64 - 1.0;
                    let next = end_display+1.0;
                    let limit = if truncation.truncated_by == Some(TruncatedBy::Lines) { String::new() } else { format!(" ({} limit)",format_size(DEFAULT_MAX_BYTES)) };
                    let output = format!("{}\n\n[Showing lines {display}-{end_display} of {}{limit}. Use offset={next} to continue.]",truncation.content,lines.len());
                    details = Some(json!({"truncation":truncation})); output
                } else if limited.is_some_and(|count| start+count < lines.len() as f64) {
                    let consumed = start+limited.unwrap();
                    format!("{}\n\n[{} more lines in file. Use offset={} to continue.]",truncation.content,lines.len() as f64-consumed,consumed+1.0)
                } else { truncation.content };
                let mut result = text_result(output); result.details = details; Ok(result)
            })
        }),
    }
}
// JS Array.slice applies ToIntegerOrInfinity, then resolves negative indices.
fn slice_index(value: f64, length: usize) -> usize {
    let value = value.trunc();
    if value < 0.0 {
        (length as f64 + value).max(0.0) as usize
    } else {
        (value as usize).min(length)
    }
}
