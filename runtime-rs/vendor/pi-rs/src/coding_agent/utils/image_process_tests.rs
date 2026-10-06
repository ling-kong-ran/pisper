//! Native codec regression tests; re-encoded bytes are not compared to Photon.
//! Deterministic surfaces (passthrough bytes, decisions, dimensions, hint
//! strings, omitted messages) are pinned against `image_oracle.json`, captured
//! from the real upstream TS sources under node with the real Photon WASM
//! backend (see `tests/fixtures/image_oracle/capture.mjs`).
use super::*;
use crate::ai::types::{ImageContent, TextContent, TextOrImageBlock};
use crate::coding_agent::agent_session::tool_result_images::normalize_tool_result_images;
use crate::coding_agent::cli::file_processor::{
    native_process_image, process_file_arguments, ProcessFileOptions, ProcessedImage,
};
use serde_json::Value;

fn oracle() -> Value {
    serde_json::from_str(include_str!("image_oracle.json")).unwrap()
}

/// Base64 fixture captured verbatim from the upstream test suite.
fn fixture_b64(name: &str) -> String {
    oracle()["fixtures"][name]
        .as_str()
        .unwrap_or_else(|| panic!("fixture {name} missing from oracle"))
        .to_string()
}

fn fixture_bytes(name: &str) -> Vec<u8> {
    crate::tui::terminal_image::base64::decode(&fixture_b64(name))
}
fn image_bytes(format: ImageFormat, w: u32, h: u32) -> Vec<u8> {
    let image = DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([(x * 31 % 256) as u8, (y * 41 % 256) as u8, 83])
    }));
    let mut buf = Cursor::new(Vec::new());
    image.write_to(&mut buf, format).unwrap();
    buf.into_inner()
}
fn success(result: ProcessImageResult) -> (String, String, Vec<String>) {
    match result {
        ProcessImageResult::Success {
            data,
            mime_type,
            hints,
        } => (data, mime_type, hints),
        other => panic!("{other:?}"),
    }
}
#[test]
fn supported_small_images_preserve_original_bytes_and_normalize_mime() {
    for (format, mime, want) in [
        (ImageFormat::Png, " image/PNG; charset=utf8", "image/png"),
        (ImageFormat::Jpeg, "image/jpg", "image/jpeg"),
        (ImageFormat::Gif, "image/gif", "image/gif"),
        (ImageFormat::WebP, "image/webp", "image/webp"),
    ] {
        let bytes = image_bytes(format, 7, 5);
        let (data, mime, hints) = success(process_image_in_process(
            bytes.clone(),
            mime,
            Default::default(),
        ));
        assert_eq!(data, encode_base64(&bytes));
        assert_eq!(mime, want);
        assert!(hints.is_empty());
    }
}
#[test]
fn bmp_conversion_is_required_even_when_auto_resize_is_disabled() {
    for auto in [true, false] {
        let (data, mime, hints) = success(process_image_in_process(
            image_bytes(ImageFormat::Bmp, 7, 5),
            "IMAGE/BMP; variant=x",
            ProcessImageOptions {
                auto_resize_images: Some(auto),
                ..Default::default()
            },
        ));
        let bytes = crate::tui::terminal_image::base64::decode(&data);
        let decoded = decode(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (7, 5));
        assert_eq!(mime, "image/png");
        assert_eq!(hints, ["[Image converted from image/bmp to image/png.]"]);
    }
}
#[test]
fn max_dimensions_use_oriented_aspect_ratio_png_first_and_exact_hint() {
    let result = process_image_in_process(
        image_bytes(ImageFormat::Jpeg, 100, 50),
        "image/jpeg",
        ProcessImageOptions {
            resize_options: Some(ImageResizeOptions {
                max_width: Some(40),
                max_height: Some(10),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    let (data, mime, hints) = success(result);
    let decoded = decode(&crate::tui::terminal_image::base64::decode(&data)).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (20, 10));
    assert_eq!(mime, "image/png");
    assert_eq!(
        hints,
        [
            "[Image: original 100x50, displayed at 20x10. Multiply coordinates by 5.00 to map to original image.]"
        ]
    );
}
#[test]
fn exif_rotation_happens_before_dimension_limiting() {
    let jpeg = image_bytes(ImageFormat::Jpeg, 2, 4);
    // APP1: Exif + little-endian TIFF, orientation tag = rotate 90 CW.
    let exif = [
        b'E', b'x', b'i', b'f', 0, 0, b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 0x01, 3, 0, 1, 0,
        0, 0, 6, 0, 0, 0, 0, 0, 0, 0,
    ];
    let mut bytes = vec![0xff, 0xd8, 0xff, 0xe1];
    bytes.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
    bytes.extend_from_slice(&exif);
    bytes.extend_from_slice(&jpeg[2..]);
    let (data, _, hints) = success(process_image_in_process(
        bytes,
        "image/jpeg",
        ProcessImageOptions {
            resize_options: Some(ImageResizeOptions {
                max_width: Some(2),
                max_height: Some(1),
                ..Default::default()
            }),
            ..Default::default()
        },
    ));
    let image = decode(&crate::tui::terminal_image::base64::decode(&data)).unwrap();
    assert_eq!((image.width(), image.height()), (2, 1));
    assert!(hints[0].contains("original 4x2, displayed at 2x1"));
}
#[test]
fn encoded_byte_limit_is_strict_and_impossible_limits_omit() {
    let bytes = image_bytes(ImageFormat::Png, 30, 20);
    let exact = bytes.len().div_ceil(3) * 4;
    let (data, _, hints) = success(process_image_in_process(
        bytes,
        "image/png",
        ProcessImageOptions {
            resize_options: Some(ImageResizeOptions {
                max_bytes: Some(exact),
                ..Default::default()
            }),
            ..Default::default()
        },
    ));
    assert!(data.len() < exact);
    assert!(!hints.is_empty());
    let result = process_image_in_process(
        image_bytes(ImageFormat::Png, 3, 3),
        "image/png",
        ProcessImageOptions {
            resize_options: Some(ImageResizeOptions {
                max_bytes: Some(1),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    assert_eq!(
        result,
        omitted("[Image omitted: could not be resized below the inline image size limit.]")
    );
}
#[test]
fn invalid_supported_and_unsupported_images_follow_distinct_failure_paths() {
    assert_eq!(
        process_image_in_process(vec![1, 2], "image/png", Default::default()),
        omitted("[Image omitted: could not be resized below the inline image size limit.]")
    );
    assert_eq!(
        process_image_in_process(vec![1, 2], "image/bmp", Default::default()),
        omitted("[Image omitted: could not be converted to a supported inline image format.]")
    );
    let (data, _, hints) = success(process_image_in_process(
        vec![1, 2],
        "image/png",
        ProcessImageOptions {
            auto_resize_images: Some(false),
            ..Default::default()
        },
    ));
    assert_eq!(data, "AQI=");
    assert!(hints.is_empty());
}
#[tokio::test]
async fn normalize_results_keeps_failed_blocks_and_inserts_hints_after_conversions() {
    let text = TextOrImageBlock::Text(TextContent {
        text: "before".into(),
        text_signature: None,
    });
    let invalid = TextOrImageBlock::Image(ImageContent {
        data: "AQI=".into(),
        mime_type: "image/png".into(),
    });
    let bmp = TextOrImageBlock::Image(ImageContent {
        data: encode_base64(&image_bytes(ImageFormat::Bmp, 4, 3)),
        mime_type: "image/bmp".into(),
    });
    let result = normalize_tool_result_images(vec![text.clone(), invalid.clone(), bmp], None).await;
    assert_eq!(result.len(), 4);
    assert_eq!(result[0], text);
    assert_eq!(result[1], invalid);
    assert!(matches!(&result[2],TextOrImageBlock::Image(i)if i.mime_type=="image/png"));
    assert!(
        matches!(&result[3],TextOrImageBlock::Text(t)if t.text=="[Image converted from image/bmp to image/png.]")
    );
}
#[tokio::test]
async fn read_and_cli_file_args_use_native_processing_and_model_note() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();
    std::fs::write(
        dir.path().join("tiny.bmp"),
        image_bytes(ImageFormat::Bmp, 4, 3),
    )
    .unwrap();
    let result = crate::coding_agent::core::tools::read::execute_read(
        &crate::coding_agent::core::tools::read::ReadToolInput {
            path: "tiny.bmp".into(),
            offset: None,
            limit: None,
        },
        cwd,
        &Default::default(),
        true,
        true,
        None,
    )
    .await
    .unwrap();
    assert_eq!(result["content"][1]["mimeType"], "image/png");
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("converted from image/bmp"));
    assert!(text.contains("Current model does not support images"));
    let cli =
        process_file_arguments(&["tiny.bmp".into()], None, native_process_image, cwd).unwrap();
    assert_eq!(cli.images.len(), 1);
    assert_eq!(cli.images[0].mime_type, "image/png");
    assert!(cli.text.contains("converted from image/bmp"));
}

// ---------------------------------------------------------------------------
// Oracle-pinned surfaces (captured from real upstream TS + Photon WASM).
// ---------------------------------------------------------------------------

/// Upstream image-processing.test.ts convertToPng suite.
#[test]
fn convert_to_png_matches_the_oracle() {
    let o = &oracle()["convertToPng"];
    // PNG passthrough is byte-exact.
    let (data, mime) = convert_to_png(&fixture_b64("TINY_PNG"), "image/png").unwrap();
    assert_eq!(data, fixture_b64("TINY_PNG"));
    assert_eq!(mime, o["pngPassthrough"]["mimeType"].as_str().unwrap());
    // JPEG conversion: mime + PNG magic byte-exact, body is backend-dependent.
    let (data, mime) = convert_to_png(&fixture_b64("TINY_JPEG"), "image/jpeg").unwrap();
    assert_eq!(mime, o["jpegToPng"]["mimeType"].as_str().unwrap());
    let bytes = crate::tui::terminal_image::base64::decode(&data);
    assert_eq!(
        &bytes[..8],
        &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]
    );
    // Undecodable bytes convert to nothing.
    assert!(convert_to_png("aGVsbG8=", "image/x-unknown").is_none());
}

/// Upstream image-processing.test.ts convertImageBytesToPng behavior (Photon
/// re-encodes even PNG input — `png_identical_to_input` is false in the
/// oracle; orientation applies after an XMP APP1 segment).
#[test]
fn convert_image_bytes_to_png_matches_the_oracle() {
    let o = &oracle()["convertImageBytesToPng"];
    let converted = convert_image_bytes_to_png(&fixture_bytes("TINY_PNG")).unwrap();
    assert_eq!(
        &converted[..8],
        &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]
    );
    assert!(!o["png_identical_to_input"].as_bool().unwrap());

    let converted = convert_image_bytes_to_png(&fixture_bytes("TINY_JPEG")).unwrap();
    assert_eq!(
        hex_upper(&converted[..8]),
        o["jpeg_magic"].as_str().unwrap().to_uppercase()
    );

    // XMP APP1 before the EXIF APP1: orientation 6 turns 1x2 into 2x1 pixels.
    let converted = convert_image_bytes_to_png(&fixture_bytes("JPEG_2X1_XMP_THEN_EXIF6")).unwrap();
    let image = decode(&converted).unwrap();
    assert_eq!(
        (image.width(), image.height()),
        (
            o["jpeg_xmp_orientation_dims"]["width"].as_u64().unwrap() as u32,
            o["jpeg_xmp_orientation_dims"]["height"].as_u64().unwrap() as u32
        )
    );

    assert!(convert_image_bytes_to_png(&[1, 2, 3]).is_none());
}

fn hex_upper(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

/// Upstream image-processing.test.ts resizeImage suite.
#[test]
fn resize_image_in_process_matches_the_oracle() {
    let o = &oracle()["resizeImage"];

    // Within limits: exact passthrough of the caller bytes.
    let tiny = resize_image_in_process(
        &fixture_bytes("TINY_PNG"),
        "image/png",
        ImageResizeOptions {
            max_width: Some(100),
            max_height: Some(100),
            max_bytes: Some(1024 * 1024),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(tiny.data, fixture_b64("TINY_PNG"));
    assert!(!tiny.was_resized);
    assert_eq!((tiny.width, tiny.height), (2, 2));
    assert_eq!(
        (tiny.original_width, tiny.original_height),
        (
            o["tiny_within_limits"]["originalWidth"].as_u64().unwrap() as u32,
            o["tiny_within_limits"]["originalHeight"].as_u64().unwrap() as u32
        )
    );

    // Dimension limits: decisions byte-exact, encoded body backend-dependent.
    let medium = resize_image_in_process(
        &fixture_bytes("MEDIUM_PNG_100x100"),
        "image/png",
        ImageResizeOptions {
            max_width: Some(50),
            max_height: Some(50),
            max_bytes: Some(1024 * 1024),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(medium.was_resized);
    assert_eq!((medium.original_width, medium.original_height), (100, 100));
    assert_eq!((medium.width, medium.height), (50, 50));
    assert_eq!(
        medium.mime_type,
        o["medium_dimensions"]["mimeType"].as_str().unwrap()
    );
    let decoded = decode(&crate::tui::terminal_image::base64::decode(&medium.data)).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (50, 50));

    // Default limits: 2x2 stays passthrough.
    let default_limits =
        resize_image_in_process(&fixture_bytes("TINY_PNG"), "image/png", Default::default())
            .unwrap();
    assert_eq!(default_limits.data, fixture_b64("TINY_PNG"));
    assert!(!default_limits.was_resized);

    // JPEG passthrough keeps the original bytes verbatim.
    let jpeg = resize_image_in_process(
        &fixture_bytes("TINY_JPEG"),
        "image/jpeg",
        ImageResizeOptions {
            max_width: Some(100),
            max_height: Some(100),
            max_bytes: Some(1024 * 1024),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!jpeg.was_resized);
    assert_eq!(jpeg.data, fixture_b64("TINY_JPEG"));
    assert_eq!((jpeg.width, jpeg.height), (2, 2));

    // Impossible limit: null upstream, None here.
    let impossible = resize_image_in_process(
        &fixture_bytes("LARGE_PNG_200x200"),
        "image/png",
        ImageResizeOptions {
            max_width: Some(2000),
            max_height: Some(2000),
            max_bytes: Some(1),
            ..Default::default()
        },
    );
    assert_eq!(impossible.is_none(), o["large_impossible"].is_null());

    // Undecodable input: null upstream, None here.
    assert!(resize_image_in_process(&[1, 2], "image/png", Default::default()).is_none());
}

/// Dimension target math is deterministic: 100x100 with max 40x10 lands on
/// 10x10 exactly as the upstream oracle run.
#[test]
fn dimension_targets_match_the_oracle() {
    let dims = resize_image_in_process(
        &fixture_bytes("MEDIUM_PNG_100x100"),
        "image/png",
        ImageResizeOptions {
            max_width: Some(40),
            max_height: Some(10),
            max_bytes: Some(1024 * 1024),
            ..Default::default()
        },
    )
    .unwrap();
    let o = &oracle()["resizeImageInProcess"]["dims_40x10_of_100x50ish"];
    assert!(dims.was_resized);
    assert_eq!((dims.width, dims.height), (10, 10));
    assert_eq!((dims.original_width, dims.original_height), (100, 100));
    assert_eq!(dims.mime_type, o["mimeType"].as_str().unwrap());
    assert!(o["wasResized"].as_bool().unwrap());
}

/// The byte-limit loop always lands below the limit with valid image bytes
/// (upstream image-processing.test.ts "should resize image exceeding byte
/// limit" — the re-encoded body itself is backend-dependent).
#[test]
fn byte_limit_loop_produces_smaller_output() {
    let original = fixture_bytes("LARGE_PNG_200x200");
    let original_size = original.len();
    let result = resize_image_in_process(
        &original,
        "image/png",
        ImageResizeOptions {
            max_width: Some(2000),
            max_height: Some(2000),
            max_bytes: Some((fixture_b64("LARGE_PNG_200x200").len() as f64 * 0.9).floor() as usize),
            ..Default::default()
        },
    )
    .unwrap();
    let result_bytes = crate::tui::terminal_image::base64::decode(&result.data);
    assert!(result_bytes.len() < original_size);
    assert!(result.data.len() < fixture_b64("LARGE_PNG_200x200").len());
    assert!(result.was_resized);
    assert_eq!((result.original_width, result.original_height), (200, 200));
}

/// Upstream formatDimensionNote suite, strings byte-exact against the oracle.
#[test]
fn format_dimension_note_matches_the_oracle_strings() {
    let o = &oracle()["formatDimensionNote"];
    let resized = |ow: u32, oh: u32, w: u32, h: u32| ResizedImage {
        data: String::new(),
        mime_type: "image/png".into(),
        original_width: ow,
        original_height: oh,
        width: w,
        height: h,
        was_resized: true,
    };
    assert!(format_dimension_note(&ResizedImage {
        was_resized: false,
        ..resized(100, 100, 100, 100)
    })
    .is_none());
    assert_eq!(
        format_dimension_note(&resized(2000, 1000, 1000, 500)).unwrap(),
        o["resized2000x1000"].as_str().unwrap()
    );
    assert_eq!(
        format_dimension_note(&resized(100, 50, 20, 10)).unwrap(),
        o["resizedNonIntegralScale"].as_str().unwrap()
    );
    assert_eq!(
        format_dimension_note(&resized(3, 2, 2, 1)).unwrap(),
        o["resized3x2to2x1"].as_str().unwrap()
    );
    // JS toFixed(2) rounds exact ties toward the larger representant (9/8).
    assert_eq!(
        format_dimension_note(&resized(9, 8, 8, 8)).unwrap(),
        "[Image: original 9x8, displayed at 8x8. Multiply coordinates by 1.13 to map to original image.]"
    );
}

/// Upstream image-process.test.ts processImage suite, hints and messages
/// byte-exact; re-encoded bodies checked at the format level only.
#[test]
fn process_image_matches_the_oracle() {
    let o = &oracle()["processImage"];
    let success = |result: ProcessImageResult| match result {
        ProcessImageResult::Success {
            data,
            mime_type,
            hints,
        } => (data, mime_type, hints),
        other => panic!("{other:?}"),
    };

    let (data, mime, hints) = success(process_image_in_process(
        fixture_bytes("BMP_1x1"),
        "image/bmp",
        Default::default(),
    ));
    assert_eq!(mime, o["bmp_auto"]["mimeType"].as_str().unwrap());
    assert_eq!(
        hints,
        o["bmp_auto"]["hints"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h.as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        &crate::tui::terminal_image::base64::decode(&data)[..4],
        &[0x89, 0x50, 0x4e, 0x47]
    );

    // auto-resize disabled still converts, and small PNGs pass through exactly.
    let (data, mime, hints) = success(process_image_in_process(
        fixture_bytes("TINY_PNG"),
        "image/png",
        ProcessImageOptions {
            auto_resize_images: Some(false),
            ..Default::default()
        },
    ));
    assert_eq!(data, fixture_b64("TINY_PNG"));
    assert_eq!(mime, "image/png");
    assert!(hints.is_empty());

    // image/jpg normalizes to image/jpeg with original bytes verbatim.
    let (data, mime, hints) = success(process_image_in_process(
        fixture_bytes("TINY_JPEG"),
        " image/JPG; charset=utf8",
        ProcessImageOptions {
            auto_resize_images: Some(false),
            ..Default::default()
        },
    ));
    assert_eq!(data, fixture_b64("TINY_JPEG"));
    assert_eq!(mime, o["jpg_alias_no_auto"]["mimeType"].as_str().unwrap());
    assert!(hints.is_empty());

    assert_eq!(
        process_image_in_process(vec![1, 2], "image/bmp", Default::default()),
        omitted(
            oracle()["processImage"]["garbage_bmp"]["message"]
                .as_str()
                .unwrap()
        )
    );
    assert_eq!(
        process_image_in_process(vec![1, 2], "image/png", Default::default()),
        omitted(
            oracle()["processImage"]["garbage_png_auto"]["message"]
                .as_str()
                .unwrap()
        )
    );
}

/// Upstream image-resize-callers.test.ts: a failing processor leaves the caller
/// with text-only output (verified through the injectable file-processor seam).
#[test]
fn file_processor_omits_images_when_the_seam_reports_failure() {
    fn failing_process_image(
        _content: &[u8],
        _mime_type: &str,
        _auto_resize_images: bool,
    ) -> ProcessedImage {
        ProcessedImage {
            ok: false,
            message: "[Image omitted: could not be resized below the inline image size limit.]"
                .into(),
            mime_type: String::new(),
            data: String::new(),
            hints: Vec::new(),
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();
    std::fs::write(dir.path().join("test.png"), fixture_bytes("TINY_PNG")).unwrap();
    let result = process_file_arguments(
        &["test.png".into()],
        Some(ProcessFileOptions {
            auto_resize_images: Some(true),
        }),
        failing_process_image,
        cwd,
    )
    .unwrap();
    assert!(result.images.is_empty());
    assert!(result.text.contains("Image omitted"));
}

/// The async wrappers (upstream worker offload + in-process fallback) agree
/// with the sync in-process results.
#[tokio::test]
async fn async_resize_and_process_match_the_in_process_results() {
    let bytes = fixture_bytes("MEDIUM_PNG_100x100");
    let sync = resize_image_in_process(
        &bytes,
        "image/png",
        ImageResizeOptions {
            max_width: Some(50),
            max_height: Some(50),
            max_bytes: Some(1024 * 1024),
            ..Default::default()
        },
    )
    .unwrap();
    let async_result = resize_image(
        bytes.clone(),
        "image/png".into(),
        ImageResizeOptions {
            max_width: Some(50),
            max_height: Some(50),
            max_bytes: Some(1024 * 1024),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    // Same decisions; encoded bytes are deterministic for the same backend.
    assert_eq!(async_result, sync);

    let processed = process_image(
        fixture_bytes("BMP_1x1"),
        "image/bmp".into(),
        Default::default(),
    )
    .await;
    assert!(matches!(processed, ProcessImageResult::Success { .. }));
}
