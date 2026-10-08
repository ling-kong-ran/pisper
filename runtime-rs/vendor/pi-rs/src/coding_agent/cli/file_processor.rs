//! Port of upstream `coding-agent/src/cli/file-processor.ts` (sha256
//! 86e6da37b408…): process `@file` CLI arguments into text content and image
//! attachments.
//!
//! Path fallbacks are shared with the builtin read tool. The production image
//! processor uses the native image backend; callers may still inject a host
//! processor. File/codec errors are returned to main for mode-safe diagnostics.

use std::path::Path;

use crate::ai::types::content::ImageContent;
use crate::coding_agent::utils::mime::detect_supported_image_mime_type_from_file;
use crate::coding_agent::utils::text::strip_bom;

/// Upstream `ProcessedFiles`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProcessedFiles {
    pub text: String,
    pub images: Vec<ImageContent>,
}

/// Upstream `ProcessFileOptions`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessFileOptions {
    /// Whether to auto-resize images. Default: true
    pub auto_resize_images: Option<bool>,
}

/// Upstream `processImage`'s result contract (from `utils/image-process.ts`).
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedImage {
    pub ok: bool,
    pub message: String,
    pub mime_type: String,
    /// Base64-encoded image data.
    pub data: String,
    /// Optional processing hints surfaced inside the text reference.
    pub hints: Vec<String>,
}

/// Injectable `processImage` seam.
pub type ProcessImageFn =
    fn(content: &[u8], mime_type: &str, auto_resize_images: bool) -> ProcessedImage;

/// Default seam implementation: embed the bytes as-is (no resize; no hints).
pub fn base64_embed_image(
    content: &[u8],
    mime_type: &str,
    _auto_resize_images: bool,
) -> ProcessedImage {
    ProcessedImage {
        ok: true,
        message: String::new(),
        mime_type: mime_type.to_string(),
        data: crate::tui::terminal_image::base64::encode(content),
        hints: Vec::new(),
    }
}

/// Failure surface of the port (upstream: stderr message + `process.exit(1)`).
#[derive(Debug, Clone, PartialEq)]
pub enum ProcessFilesError {
    FileNotFound(String),
    ReadFailed { path: String, message: String },
}

impl std::fmt::Display for ProcessFilesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProcessFilesError::FileNotFound(path) => write!(f, "Error: File not found: {path}"),
            ProcessFilesError::ReadFailed { path, message } => {
                write!(f, "Error: Could not read file {path}: {message}")
            }
        }
    }
}

impl std::error::Error for ProcessFilesError {}

/// Shared upstream path fallback order, including NFD and macOS screenshots.
pub fn resolve_read_path(file_path: &str, cwd: &str) -> String {
    crate::coding_agent::core::tools::path_utils::resolve_read_path(file_path, cwd)
        .unwrap_or_else(|_| file_path.to_owned())
}

/// Production native image processing, shared with read and tool results.
pub fn native_process_image(
    content: &[u8],
    mime_type: &str,
    auto_resize_images: bool,
) -> ProcessedImage {
    use crate::coding_agent::utils::image_process::{
        process_image_in_process, ProcessImageOptions, ProcessImageResult,
    };
    match process_image_in_process(
        content.to_vec(),
        mime_type,
        ProcessImageOptions {
            auto_resize_images: Some(auto_resize_images),
            ..Default::default()
        },
    ) {
        ProcessImageResult::Success {
            data,
            mime_type,
            hints,
        } => ProcessedImage {
            ok: true,
            message: String::new(),
            mime_type,
            data,
            hints,
        },
        ProcessImageResult::Omitted { message } => ProcessedImage {
            ok: false,
            message,
            mime_type: String::new(),
            data: String::new(),
            hints: Vec::new(),
        },
    }
}

/// Upstream `processFileArguments`.
pub fn process_file_arguments(
    file_args: &[String],
    options: Option<ProcessFileOptions>,
    process_image: ProcessImageFn,
    cwd: &str,
) -> Result<ProcessedFiles, ProcessFilesError> {
    let auto_resize_images = options
        .and_then(|options| options.auto_resize_images)
        .unwrap_or(true);
    let mut text = String::new();
    let mut images: Vec<ImageContent> = Vec::new();

    for file_arg in file_args {
        // Expand and resolve path (handles ~ expansion and macOS screenshot
        // Unicode spaces)
        let absolute_path = resolve_read_path(file_arg, cwd);

        // Check if file exists
        if !Path::new(&absolute_path).exists() {
            return Err(ProcessFilesError::FileNotFound(absolute_path));
        }

        // Check if file is empty
        let metadata = std::fs::metadata(&absolute_path)
            .map_err(|_| ProcessFilesError::FileNotFound(absolute_path.clone()))?;
        if metadata.len() == 0 {
            // Skip empty files
            continue;
        }

        let mime_type = detect_supported_image_mime_type_from_file(&absolute_path)
            .ok()
            .flatten();

        if let Some(mime_type) = mime_type {
            // Handle image file
            let content =
                std::fs::read(&absolute_path).map_err(|error| ProcessFilesError::ReadFailed {
                    path: absolute_path.clone(),
                    message: error.to_string(),
                })?;
            let processed = process_image(&content, mime_type, auto_resize_images);

            if !processed.ok {
                text.push_str(&format!(
                    "<file name=\"{}\">{}</file>\n",
                    absolute_path, processed.message
                ));
                continue;
            }

            images.push(ImageContent {
                mime_type: processed.mime_type.clone(),
                data: processed.data.clone(),
            });

            // Add text reference to image with optional processing hints
            if !processed.hints.is_empty() {
                text.push_str(&format!(
                    "<file name=\"{}\">{}</file>\n",
                    absolute_path,
                    processed.hints.join("\n")
                ));
            } else {
                text.push_str(&format!("<file name=\"{}\"></file>\n", absolute_path));
            }
        } else {
            // Handle text file
            match std::fs::read(&absolute_path) {
                Ok(bytes) => {
                    // Node Buffer decoding replaces invalid UTF-8, then strips one BOM.
                    let content = String::from_utf8_lossy(&bytes);
                    text.push_str(&format!(
                        "<file name=\"{}\">\n{}\n</file>\n",
                        absolute_path,
                        strip_bom(&content)
                    ));
                }
                Err(error) => {
                    return Err(ProcessFilesError::ReadFailed {
                        path: absolute_path,
                        message: error.to_string(),
                    });
                }
            }
        }
    }

    Ok(ProcessedFiles { text, images })
}

#[cfg(test)]
#[path = "file_processor_tests.rs"]
mod tests;
