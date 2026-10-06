//! Upstream tool-result-images.ts: normalize images once at history ingress.
use crate::ai::types::{ImageContent, TextContent, TextOrImageBlock};
use crate::coding_agent::utils::image_process::{
    process_image, ProcessImageOptions, ProcessImageResult,
};
#[derive(Debug, Clone, Copy, Default)]
pub struct NormalizeToolResultImagesOptions {
    pub auto_resize_images: Option<bool>,
}
pub async fn normalize_tool_result_images(
    content: Vec<TextOrImageBlock>,
    options: Option<NormalizeToolResultImagesOptions>,
) -> Vec<TextOrImageBlock> {
    if !content
        .iter()
        .any(|b| matches!(b, TextOrImageBlock::Image(_)))
    {
        return content;
    }
    let options = ProcessImageOptions {
        auto_resize_images: options.and_then(|o| o.auto_resize_images),
        ..Default::default()
    };
    let mut result = Vec::new();
    for block in content {
        let TextOrImageBlock::Image(ref image) = block else {
            result.push(block);
            continue;
        };
        let processed = process_image(
            crate::tui::terminal_image::base64::decode(&image.data),
            image.mime_type.clone(),
            options,
        )
        .await;
        match processed {
            ProcessImageResult::Omitted { .. } => result.push(block),
            ProcessImageResult::Success {
                data,
                mime_type,
                hints,
            } if data == image.data && mime_type == image.mime_type && hints.is_empty() => {
                result.push(block)
            }
            ProcessImageResult::Success {
                data,
                mime_type,
                hints,
            } => {
                result.push(TextOrImageBlock::Image(ImageContent { data, mime_type }));
                if !hints.is_empty() {
                    result.push(TextOrImageBlock::Text(TextContent {
                        text: hints.join("\n"),
                        text_signature: None,
                    }));
                }
            }
        }
    }
    result
}
