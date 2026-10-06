//! Upstream read.ts: Buffer UTF-8 semantics, bounded reads and image attachments.
use super::path_utils::resolve_read_path_async;
use super::truncate::*;
use super::{cancellable, check_abort, context_cwd, definition, text_result, ToolFuture};
use crate::coding_agent::extensions::types::{AbortSignal, ToolDefinition};
use crate::coding_agent::utils::image_process::{
    process_image, ProcessImageOptions, ProcessImageResult,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadToolInput {
    pub path: String,
    pub offset: Option<f64>,
    pub limit: Option<f64>,
}
pub type DetectImageMimeType = Arc<dyn Fn(String) -> ToolFuture<Option<String>> + Send + Sync>;

#[derive(Clone)]
pub struct ReadOperations {
    pub read_file: Arc<dyn Fn(String) -> ToolFuture<Vec<u8>> + Send + Sync>,
    pub access: Arc<dyn Fn(String) -> ToolFuture<()> + Send + Sync>,
    pub detect_image_mime_type: Option<DetectImageMimeType>,
}
impl Default for ReadOperations {
    fn default() -> Self {
        Self {
            read_file: Arc::new(|p| {
                Box::pin(async move {
                    tokio::fs::read(&p)
                        .await
                        .map_err(|e| super::io_error(e, "open", &p))
                })
            }),
            access: Arc::new(|p| {
                Box::pin(async move {
                    tokio::fs::metadata(&p)
                        .await
                        .map(|_| ())
                        .map_err(|e| super::io_error(e, "access", &p))
                })
            }),
            detect_image_mime_type: Some(Arc::new(|p| {
                Box::pin(async move {
                    tokio::task::spawn_blocking(move || crate::coding_agent::utils::mime::detect_supported_image_mime_type_from_file(&p).map(|s|s.map(str::to_owned)).map_err(|e|super::io_error(e,"open",&p))).await.map_err(|e|e.to_string())?
                })
            })),
        }
    }
}
#[derive(Clone, Default)]
pub struct ReadToolOptions {
    pub auto_resize_images: Option<bool>,
    pub operations: Option<ReadOperations>,
}
pub fn create_read_tool_definition(cwd: &str, options: ReadToolOptions) -> Arc<ToolDefinition> {
    let mut def=definition("read",&format!("Read the contents of a file. Supports text files and images (jpg, png, gif, webp, bmp). Images are sent as attachments. For text files, output is truncated to {DEFAULT_MAX_LINES} lines or {}KB (whichever is hit first). Use offset/limit for large files. When you need the full file, continue with offset until complete.",DEFAULT_MAX_BYTES/1024),"Read file contents",&["Use read to examine files instead of cat or sed."],json!({"type":"object","properties":{"path":{"type":"string","description":"Path to the file to read (relative or absolute)"},"offset":{"type":"number","description":"Line number to start reading from (1-indexed)"},"limit":{"type":"number","description":"Maximum number of lines to read"}},"required":["path"]}),true);
    let cwd = cwd.to_owned();
    let ops = options.operations.unwrap_or_default();
    let auto_resize = options.auto_resize_images.unwrap_or(true);
    def.execute_async = Some(Arc::new(move |_, args, signal, _, ctx| {
        let cwd = cwd.clone();
        let ops = ops.clone();
        Box::pin(async move {
            let input: ReadToolInput = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let cwd = context_cwd(&ctx, &cwd)?;
            let model = ctx.model()?;
            let non_vision = model.as_ref().is_some_and(|m| {
                !m["input"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|v| v == "image"))
            });
            execute_read(&input, &cwd, &ops, auto_resize, non_vision, signal.as_ref()).await
        })
    }));
    Arc::new(def)
}
pub async fn execute_read(
    input: &ReadToolInput,
    cwd: &str,
    ops: &ReadOperations,
    auto_resize: bool,
    non_vision: bool,
    signal: Option<&Arc<AbortSignal>>,
) -> Result<Value, String> {
    cancellable(signal,async {
        let path=resolve_read_path_async(&input.path,cwd).await?;
        check_abort(signal)?;(ops.access)(path.clone()).await?;check_abort(signal)?;
        let mime=match &ops.detect_image_mime_type { Some(detect)=>detect(path.clone()).await?,None=>None };
        let bytes=(ops.read_file)(path).await?;
        let result=if let Some(mime)=mime {
            let note=if non_vision { "\n[Current model does not support images. The image will be omitted from this request.]" } else { "" };
            match process_image(bytes,mime.clone(),ProcessImageOptions{auto_resize_images:Some(auto_resize),..Default::default()}).await {
                ProcessImageResult::Omitted{message}=>text_result(format!("Read image file [{mime}]\n{message}{note}")),
                ProcessImageResult::Success{data,mime_type,hints}=>{
                    let hints=if hints.is_empty(){String::new()}else{format!("\n{}",hints.join("\n"))};
                    json!({"content":[{"type":"text","text":format!("Read image file [{mime_type}]{hints}{note}")},{"type":"image","data":data,"mimeType":mime_type}]})
                }
            }
        } else { read_text(&String::from_utf8_lossy(&bytes),input)? };
        check_abort(signal)?;Ok(result)
    }).await
}
fn slice_index(value: f64, len: usize) -> usize {
    let value = value.trunc();
    if value < 0.0 {
        (len as f64 + value).max(0.0) as usize
    } else {
        (value as usize).min(len)
    }
}
pub fn read_text(text: &str, input: &ReadToolInput) -> Result<Value, String> {
    // Buffer.toString keeps the BOM (unlike the harness TextDecoder path).
    let lines: Vec<_> = text.split('\n').collect();
    let start = input
        .offset
        .filter(|v| *v != 0.0)
        .map(|v| (v - 1.0).max(0.0))
        .unwrap_or(0.0);
    let display = start + 1.0;
    if start >= lines.len() as f64 {
        return Err(format!(
            "Offset {} is beyond end of file ({} lines total)",
            input.offset.unwrap_or(0.0),
            lines.len()
        ));
    }
    let end = input
        .limit
        .map(|limit| (start + limit).min(lines.len() as f64));
    let begin = slice_index(start, lines.len());
    let stop = end
        .map(|n| slice_index(n, lines.len()))
        .unwrap_or(lines.len())
        .max(begin);
    let truncation = truncate_head(&lines[begin..stop].join("\n"), TruncationOptions::default());
    let details = (truncation.first_line_exceeds_limit || truncation.truncated)
        .then(|| json!({"truncation":as_value(&truncation)}));
    let output = if truncation.first_line_exceeds_limit {
        format!("[Line {display} is {}, exceeds {} limit. Use bash: sed -n '{display}p' {} | head -c {DEFAULT_MAX_BYTES}]",format_size(lines[begin].len() as u64),format_size(DEFAULT_MAX_BYTES),input.path)
    } else if truncation.truncated {
        let last = display + truncation.output_lines as f64 - 1.0;
        let next = last + 1.0;
        let limit = if matches!(
            truncation.truncated_by,
            Some(crate::agent_core::harness::TruncatedBy::Lines)
        ) {
            String::new()
        } else {
            format!(" ({} limit)", format_size(DEFAULT_MAX_BYTES))
        };
        format!(
            "{}\n\n[Showing lines {display}-{last} of {}{limit}. Use offset={next} to continue.]",
            truncation.content,
            lines.len()
        )
    } else if end.is_some_and(|end| end < lines.len() as f64) {
        let consumed = end.unwrap();
        format!(
            "{}\n\n[{} more lines in file. Use offset={} to continue.]",
            truncation.content,
            lines.len() as f64 - consumed,
            consumed + 1.0
        )
    } else {
        truncation.content
    };
    let mut result = text_result(output);
    if let Some(details) = details {
        result["details"] = details;
    }
    Ok(result)
}
