//! Port of upstream `coding-agent/src/utils/image-process.ts` (sha256
//! fffb65fba877…), `image-resize-core.ts` (b35233293974…), `image-resize.ts`
//! (d93a007eb45a…) and `image-convert.ts` (791f2a134995…) native
//! implementation.
//!
//! Branches, MIME normalization, limits, hint composition, EXIF orientation
//! (see [`super::exif_orientation`]) and candidate ordering follow upstream
//! byte-for-byte. Photon WASM is replaced with the vendored `image` crate:
//! passthrough bytes are exact; re-encoded PNG/JPEG bytes are backend-dependent
//! and are asserted at the format level (magic, dimensions, decisions) rather
//! than byte-identically to Photon. Dimension math, the quality-step ladder,
//! candidate selection order, encoded-size thresholds and all hint strings are
//! deterministic and oracle-pinned (see `image_oracle.json`).
//!
//! Upstream `image-resize.ts` runs Photon inside a node worker thread and falls
//! back to in-process resizing; the Rust mapping is `tokio`'s blocking pool
//! ([`resize_image`]/[`process_image`]), which serves the same "never block the
//! event loop" contract, so no separate worker module exists.
use crate::agent_core::harness::tools::image::encode_base64;
use crate::coding_agent::utils::exif_orientation::apply_orientation;
use image::{imageops::FilterType, DynamicImage, ImageBuffer, ImageFormat, ImageReader};
use image_native as image;
use std::io::Cursor;

/// Upstream `ImageResizeOptions`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ImageResizeOptions {
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub max_bytes: Option<usize>,
    pub jpeg_quality: Option<u8>,
}

/// Upstream `ProcessImageOptions`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessImageOptions {
    pub auto_resize_images: Option<bool>,
    pub resize_options: Option<ImageResizeOptions>,
}

/// Upstream `ProcessImageResult`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessImageResult {
    Success {
        data: String,
        mime_type: String,
        hints: Vec<String>,
    },
    Omitted {
        message: String,
    },
}

/// Upstream `ResizedImage` (from `image-resize-core.ts`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResizedImage {
    /// base64
    pub data: String,
    pub mime_type: String,
    pub original_width: u32,
    pub original_height: u32,
    pub width: u32,
    pub height: u32,
    pub was_resized: bool,
}

// 4.5MB of base64 payload. Provides headroom below Anthropic's 5MB limit.
const DEFAULT_MAX_BYTES: usize = 4_718_592;

const OMITTED_CONVERSION: &str =
    "[Image omitted: could not be converted to a supported inline image format.]";
const OMITTED_RESIZE: &str =
    "[Image omitted: could not be resized below the inline image size limit.]";

/// Decode bytes to raw RGBA without applying any orientation, mirroring
/// Photon's `new_from_byteslice` (orientation is applied separately by
/// [`apply_orientation`] from the original bytes).
fn decode_rgba(bytes: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let image = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let rgba = image.to_rgba8();
    let (width, height) = (rgba.width(), rgba.height());
    Some((rgba.into_raw(), width, height))
}

/// Decode to an RGBA [`DynamicImage`] with upstream EXIF orientation applied.
fn decode(bytes: &[u8]) -> Option<DynamicImage> {
    let (mut rgba, mut width, mut height) = decode_rgba(bytes)?;
    apply_orientation(&mut rgba, &mut width, &mut height, bytes);
    Some(DynamicImage::ImageRgba8(ImageBuffer::from_raw(
        width, height, rgba,
    )?))
}

fn png(image: &DynamicImage) -> Option<Vec<u8>> {
    let mut output = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image.to_rgba8())
        .write_to(&mut output, ImageFormat::Png)
        .ok()?;
    Some(output.into_inner())
}

fn omitted(message: &str) -> ProcessImageResult {
    ProcessImageResult::Omitted {
        message: message.into(),
    }
}

/// Upstream `baseMimeType`.
fn base_mime_type(mime_type: &str) -> String {
    mime_type
        .split(';')
        .next()
        .unwrap_or(mime_type)
        .trim()
        .to_lowercase()
}

/// Upstream `normalizeSupportedImageMimeType`.
fn normalize_supported_image_mime_type(mime_type: &str) -> Option<&'static str> {
    match base_mime_type(mime_type).as_str() {
        "image/png" => Some("image/png"),
        "image/jpeg" | "image/jpg" => Some("image/jpeg"),
        "image/gif" => Some("image/gif"),
        "image/webp" => Some("image/webp"),
        _ => None,
    }
}

struct NormalizedImage {
    bytes: Vec<u8>,
    mime_type: &'static str,
    converted_from: Option<String>,
}

/// Upstream `normalizeImage`.
fn normalize_image(bytes: &[u8], mime_type: &str) -> Option<NormalizedImage> {
    if let Some(mime_type) = normalize_supported_image_mime_type(mime_type) {
        return Some(NormalizedImage {
            bytes: bytes.to_vec(),
            mime_type,
            converted_from: None,
        });
    }

    let png_bytes = convert_image_bytes_to_png(bytes)?;
    Some(NormalizedImage {
        bytes: png_bytes,
        mime_type: "image/png",
        converted_from: Some(base_mime_type(mime_type)),
    })
}

/// Upstream `conversionHint`.
fn conversion_hint(from: Option<&str>, to: &str) -> Option<String> {
    let from = from?;
    if from == to {
        return None;
    }
    Some(format!("[Image converted from {from} to {to}.]"))
}

/// Upstream `convertImageBytesToPng`: decode (ignoring container orientation),
/// re-orient from the original bytes, and re-encode as PNG. Always re-encodes,
/// even for PNG input (Photon behavior: `png_identical_to_input` is false in
/// the oracle). Returns `None` when the bytes cannot be decoded as an image.
pub fn convert_image_bytes_to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let image = decode(bytes)?;
    png(&image)
}

/// Upstream `convertToPng`: PNG input passes through verbatim; everything else
/// is decoded and re-encoded as PNG. Returns `(base64 data, mime type)`.
pub fn convert_to_png(base64_data: &str, mime_type: &str) -> Option<(String, String)> {
    // Already PNG, no conversion needed
    if mime_type == "image/png" {
        return Some((base64_data.to_string(), mime_type.to_string()));
    }

    let bytes = crate::tui::terminal_image::base64::decode(base64_data);
    let png_bytes = convert_image_bytes_to_png(&bytes)?;
    Some((encode_base64(&png_bytes), "image/png".to_string()))
}

/// Upstream `ImageTranscoder` (from `@earendil-works/pi-tui`): base64 image in,
/// base64 PNG out (`None` when the bytes cannot be decoded).
pub type PngTranscoder = std::sync::Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Upstream `loadPngTranscoder` (v1.0.0 `image-convert.ts`): a synchronous PNG
/// transcoder over the native decode/re-encode path. Upstream returns
/// `undefined` when photon cannot load; the native port always decodes, so the
/// `Option` tracks the decode failure of an individual image instead.
pub fn load_png_transcoder() -> Option<PngTranscoder> {
    Some(std::sync::Arc::new(|base64_data: &str| {
        let bytes = crate::tui::terminal_image::base64::decode(base64_data);
        convert_image_bytes_to_png(&bytes).map(|png_bytes| encode_base64(&png_bytes))
    }))
}

static PNG_TRANSCODER: std::sync::OnceLock<PngTranscoder> = std::sync::OnceLock::new();
static PNG_TRANSCODER_REGISTERED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Read the transcoder registered by [`ensure_png_transcoder`].
pub fn registered_png_transcoder() -> Option<&'static PngTranscoder> {
    PNG_TRANSCODER.get()
}

/// Upstream `ensurePngTranscoder` (v1.0.0 `image-convert.ts`): on Kitty-protocol
/// terminals, register the PNG transcoder once so non-PNG images render, then run
/// `on_registered`. Not called when already registered or the terminal is not
/// kitty. Upstream registers into pi-tui's global `setImageTranscoder` slot; the
/// Rust tui kitty path decodes in-process and has no such slot, so registration
/// lands in this module's slot (readable via [`registered_png_transcoder`]) with
/// identical once/kitty gating and callback semantics.
pub fn ensure_png_transcoder(on_registered: impl FnOnce()) {
    use std::sync::atomic::Ordering;
    if PNG_TRANSCODER_REGISTERED.load(Ordering::Acquire)
        || crate::tui::terminal_image::get_capabilities().images != Some("kitty")
    {
        return;
    }
    let Some(transcoder) = load_png_transcoder() else {
        return;
    };
    if PNG_TRANSCODER.set(transcoder).is_ok() {
        PNG_TRANSCODER_REGISTERED.store(true, Ordering::Release);
        on_registered();
    }
}

/// Upstream `resizeImageInProcess`: resize to fit within the max dimensions and
/// encoded file size. Returns `None` when the image cannot be decoded or cannot
/// be resized below `max_bytes`.
///
/// Strategy (upstream order): first resize to maxWidth/maxHeight, try PNG then
/// JPEG at decreasing qualities picking the first candidate under maxBytes,
/// then progressively reduce dimensions by 25% until 1x1.
pub fn resize_image_in_process(
    input_bytes: &[u8],
    mime_type: &str,
    options: ImageResizeOptions,
) -> Option<ResizedImage> {
    let max_width = options.max_width.unwrap_or(2000);
    let max_height = options.max_height.unwrap_or(2000);
    let max_bytes = options.max_bytes.unwrap_or(DEFAULT_MAX_BYTES);
    let input_base64_size = input_bytes.len().div_ceil(3) * 4;
    let format = mime_type.split('/').nth(1).unwrap_or("png");
    let passthrough_mime = if mime_type.is_empty() {
        format!("image/{format}")
    } else {
        mime_type.to_string()
    };

    let image = decode(input_bytes)?;
    let (original_width, original_height) = (image.width(), image.height());

    // Check if already within all limits (dimensions AND encoded size)
    if original_width <= max_width && original_height <= max_height && input_base64_size < max_bytes
    {
        return Some(ResizedImage {
            data: encode_base64(input_bytes),
            mime_type: passthrough_mime,
            original_width,
            original_height,
            width: original_width,
            height: original_height,
            was_resized: false,
        });
    }

    // Calculate initial dimensions respecting max limits
    let mut width = original_width;
    let mut height = original_height;
    if width > max_width {
        height = (f64::from(height) * f64::from(max_width) / f64::from(width)).round() as u32;
        width = max_width;
    }
    if height > max_height {
        width = (f64::from(width) * f64::from(max_height) / f64::from(height)).round() as u32;
        height = max_height;
    }
    if width == 0 || height == 0 {
        return None;
    }

    let mut quality_steps = Vec::new();
    for quality in [options.jpeg_quality.unwrap_or(80), 85, 70, 55, 40] {
        if !quality_steps.contains(&quality) {
            quality_steps.push(quality);
        }
    }

    loop {
        let resized = DynamicImage::ImageRgba8(image::imageops::resize(
            &image.to_rgba8(),
            width,
            height,
            FilterType::Lanczos3,
        ));
        let png_bytes = png(&resized)?;
        let mut candidates = vec![(png_bytes, "image/png")];
        let rgb = resized.to_rgb8();
        for quality in &quality_steps {
            let mut output = Vec::new();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output, *quality)
                .encode_image(&rgb)
                .ok()?;
            candidates.push((output, "image/jpeg"));
        }

        for (bytes, mime) in candidates {
            if bytes.len().div_ceil(3) * 4 < max_bytes {
                return Some(ResizedImage {
                    data: encode_base64(&bytes),
                    mime_type: mime.to_string(),
                    original_width,
                    original_height,
                    width,
                    height,
                    was_resized: true,
                });
            }
        }

        if width == 1 && height == 1 {
            break;
        }

        let next_width = if width == 1 {
            1
        } else {
            ((f64::from(width) * 0.75).floor() as u32).max(1)
        };
        let next_height = if height == 1 {
            1
        } else {
            ((f64::from(height) * 0.75).floor() as u32).max(1)
        };
        if next_width == width && next_height == height {
            break;
        }

        width = next_width;
        height = next_height;
    }

    None
}

/// Format a float like JS `Number.prototype.toFixed(2)` (ties round toward the
/// larger representant, unlike Rust's half-to-even `{:.2}`).
fn js_to_fixed2(value: f64) -> String {
    let scaled = value * 100.0;
    if (scaled - scaled.round()).abs() == 0.5 {
        let n = (scaled + 0.5).floor();
        return format!("{}.{:02}", (n / 100.0).floor() as i64, (n % 100.0) as i64);
    }
    format!("{value:.2}")
}

/// Upstream `formatDimensionNote`: helps the model understand the coordinate
/// mapping. `None` for non-resized images.
pub fn format_dimension_note(result: &ResizedImage) -> Option<String> {
    if !result.was_resized {
        return None;
    }

    let scale = f64::from(result.original_width) / f64::from(result.width);
    Some(format!(
        "[Image: original {}x{}, displayed at {}x{}. Multiply coordinates by {} to map to original image.]",
        result.original_width,
        result.original_height,
        result.width,
        result.height,
        js_to_fixed2(scale)
    ))
}

/// Upstream `resizeImage`: `resizeImageInProcess` off the async event loop
/// (upstream uses a node worker thread with in-process fallback).
pub async fn resize_image(
    input_bytes: Vec<u8>,
    mime_type: String,
    options: ImageResizeOptions,
) -> Option<ResizedImage> {
    tokio::task::spawn_blocking(move || resize_image_in_process(&input_bytes, &mime_type, options))
        .await
        .ok()
        .flatten()
}

/// Upstream `processImage`.
pub fn process_image_in_process(
    bytes: Vec<u8>,
    mime_type: &str,
    options: ProcessImageOptions,
) -> ProcessImageResult {
    let auto_resize_images = options.auto_resize_images.unwrap_or(true);
    let Some(normalized) = normalize_image(&bytes, mime_type) else {
        return omitted(OMITTED_CONVERSION);
    };

    if !auto_resize_images {
        return ProcessImageResult::Success {
            data: encode_base64(&normalized.bytes),
            mime_type: normalized.mime_type.to_string(),
            hints: conversion_hint(normalized.converted_from.as_deref(), normalized.mime_type)
                .into_iter()
                .collect(),
        };
    }

    let Some(resized) = resize_image_in_process(
        &normalized.bytes,
        normalized.mime_type,
        options.resize_options.unwrap_or_default(),
    ) else {
        return omitted(OMITTED_RESIZE);
    };

    let mut hints = Vec::new();
    if let Some(converted_hint) =
        conversion_hint(normalized.converted_from.as_deref(), &resized.mime_type)
    {
        hints.push(converted_hint);
    }
    if let Some(dimension_note) = format_dimension_note(&resized) {
        hints.push(dimension_note);
    }

    ProcessImageResult::Success {
        data: resized.data,
        mime_type: resized.mime_type,
        hints,
    }
}

/// Async [`process_image_in_process`] on the blocking pool (upstream awaits the
/// worker-based `resizeImage`).
pub async fn process_image(
    bytes: Vec<u8>,
    mime_type: String,
    options: ProcessImageOptions,
) -> ProcessImageResult {
    tokio::task::spawn_blocking(move || process_image_in_process(bytes, &mime_type, options))
        .await
        .unwrap_or_else(|_| omitted(OMITTED_RESIZE))
}

#[cfg(test)]
#[path = "image_process_tests.rs"]
mod tests;
