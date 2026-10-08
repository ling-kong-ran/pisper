//! Oracle-pinned tests for the pure EXIF orientation surface.
//!
//! Every scenario below was executed against the REAL upstream
//! `exif-orientation.ts` under node with the real Photon WASM backend (see
//! `tests/fixtures/image_oracle/capture.mjs`); `image_oracle.json` holds the captured
//! dimensions and raw pixel buffers. The Rust parser must reproduce both the
//! parsed orientation value and the pixel permutation byte-for-byte.

use super::*;
use serde_json::Value;

fn oracle() -> Value {
    serde_json::from_str(include_str!("image_oracle.json")).unwrap()
}

/// The 2x3 RGBA grid used by the capture: pixel p -> [16+3p, 32+3p, 48+3p, 255].
fn grid_2x3() -> Vec<u8> {
    let mut px = vec![0u8; 2 * 3 * 4];
    for (p, px) in px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let p = p as u8;
        px[0] = 16 + 3 * p;
        px[1] = 32 + 3 * p;
        px[2] = 48 + 3 * p;
        px[3] = 255;
    }
    px
}

fn app1(payload: &[u8]) -> Vec<u8> {
    let mut segment = Vec::with_capacity(payload.len() + 4);
    segment.extend_from_slice(&[0xff, 0xe1]);
    segment.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    segment.extend_from_slice(payload);
    segment
}

struct TiffSpec {
    order: &'static str,
    value: u16,
    with_exif_prefix: bool,
    entry_count: Option<u16>,
    ifd_offset: u32,
    extra_entry: bool,
}

impl TiffSpec {
    fn le(value: u16) -> Self {
        Self {
            order: "II",
            value,
            with_exif_prefix: true,
            entry_count: None,
            ifd_offset: 8,
            extra_entry: false,
        }
    }
    fn be(value: u16) -> Self {
        Self {
            order: "MM",
            value,
            ..Self::le(value)
        }
    }
}

fn tiff(spec: TiffSpec) -> Vec<u8> {
    let le = spec.order == "II";
    let mut body = Vec::new();
    body.extend_from_slice(spec.order.as_bytes());
    body.extend_from_slice(if le { &[0x2a, 0x00] } else { &[0x00, 0x2a] });
    body.extend_from_slice(&if le {
        spec.ifd_offset.to_le_bytes()
    } else {
        spec.ifd_offset.to_be_bytes()
    });
    let count = spec.entry_count.unwrap_or(u16::from(spec.extra_entry) + 1);
    body.extend_from_slice(&if le {
        count.to_le_bytes()
    } else {
        count.to_be_bytes()
    });
    if spec.extra_entry {
        // Non-orientation entry exactly as captured (Make, tag bytes 0x01 0x0f).
        let mut make = vec![0u8; 12];
        make[0] = 0x01;
        make[1] = 0x0f;
        make[3] = 0x02;
        body.extend_from_slice(&make);
    }
    let mut entry = vec![0u8; 12];
    if le {
        entry[0] = 0x12;
        entry[1] = 0x01; // tag 0x0112 little-endian
        entry[2] = 0x03;
        entry[4] = 0x01;
        entry[8..10].copy_from_slice(&spec.value.to_le_bytes());
    } else {
        entry[0] = 0x01;
        entry[1] = 0x12; // tag 0x0112 big-endian
        entry[3] = 0x03;
        entry[5] = 0x01;
        entry[8..10].copy_from_slice(&spec.value.to_be_bytes());
    }
    body.extend_from_slice(&entry);
    body.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    if spec.with_exif_prefix {
        let mut out = b"Exif\x00\x00".to_vec();
        out.extend_from_slice(&body);
        out
    } else {
        body
    }
}

fn jpeg_with_segments(segments: Vec<Vec<u8>>) -> Vec<u8> {
    let mut bytes = vec![0xff, 0xd8];
    for segment in segments {
        bytes.extend_from_slice(&segment);
    }
    bytes.push(0x00);
    bytes
}

fn webp_with_chunks(chunks: Vec<(Vec<u8>, Vec<u8>)>) -> Vec<u8> {
    let mut body = Vec::new();
    for (id, data) in chunks {
        body.extend_from_slice(&id);
        body.extend_from_slice(&(data.len() as u32).to_le_bytes());
        body.extend_from_slice(&data);
        if data.len() % 2 == 1 {
            body.push(0x00); // RIFF pad
        }
    }
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"WEBP");
    bytes.extend_from_slice(&body);
    bytes
}

fn xmp_app1() -> Vec<u8> {
    app1(b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>")
}

/// The exact fixture matrix captured by tests/fixtures/image_oracle/capture.mjs.
fn fixture(name: &str) -> Vec<u8> {
    let comment = vec![0xff, 0xe0, 0x00, 0x04, 0x00, 0x00];
    match name {
        "jpeg_le_1" | "jpeg_le_2" | "jpeg_le_3" | "jpeg_le_4" | "jpeg_le_5" | "jpeg_le_6"
        | "jpeg_le_7" | "jpeg_le_8" => jpeg_with_segments(vec![app1(&tiff(TiffSpec::le(
            name[name.len() - 1..].parse().unwrap(),
        )))]),
        "jpeg_be_6" => jpeg_with_segments(vec![app1(&tiff(TiffSpec::be(6)))]),
        "jpeg_be_2" => jpeg_with_segments(vec![app1(&tiff(TiffSpec::be(2)))]),
        "jpeg_le_8_extra_entry" => jpeg_with_segments(vec![app1(&tiff(TiffSpec {
            extra_entry: true,
            ..TiffSpec::le(8)
        }))]),
        "jpeg_truncated_tiff" => jpeg_with_segments(vec![app1(b"Exif\x00\x00II\x2a\x00\x08\x00")]),
        "jpeg_ifd_beyond_len" => jpeg_with_segments(vec![app1(&tiff(TiffSpec {
            with_exif_prefix: false,
            ifd_offset: 0x7fff,
            ..TiffSpec::le(6)
        }))]),
        "jpeg_entry_beyond_len" => jpeg_with_segments(vec![app1(&tiff(TiffSpec {
            entry_count: Some(200),
            ..TiffSpec::le(4)
        }))]),
        "jpeg_xmp_then_exif_6" => {
            jpeg_with_segments(vec![xmp_app1(), app1(&tiff(TiffSpec::le(6)))])
        }
        "jpeg_app1_xmp_only" => jpeg_with_segments(vec![xmp_app1()]),
        "jpeg_le_9" => jpeg_with_segments(vec![app1(&tiff(TiffSpec::le(9)))]),
        "jpeg_le_0" => jpeg_with_segments(vec![app1(&tiff(TiffSpec::le(0)))]),
        "jpeg_ff_run_before_app1_3" => jpeg_with_segments(vec![
            vec![0xff, 0xff],
            comment,
            app1(&tiff(TiffSpec::le(3))),
        ]),
        "jpeg_no_markers" => vec![0xff, 0xd8, 0x01, 0x02],
        "jpeg_app1_short" => jpeg_with_segments(vec![vec![0xff, 0xe1, 0x00, 0x02, 0xaa, 0xbb]]),
        "webp_exif_prefix_6" => webp_with_chunks(vec![
            (b"VP8 ".to_vec(), vec![1, 2, 3]),
            (b"EXIF".to_vec(), tiff(TiffSpec::le(6))),
        ]),
        "webp_exif_prefix_8" => webp_with_chunks(vec![(b"EXIF".to_vec(), tiff(TiffSpec::le(8)))]),
        "webp_no_prefix_5" => webp_with_chunks(vec![(
            b"EXIF".to_vec(),
            tiff(TiffSpec {
                with_exif_prefix: false,
                ..TiffSpec::le(5)
            }),
        )]),
        "webp_odd_padding_7" => webp_with_chunks(vec![
            (b"ODDC".to_vec(), vec![9, 9, 9]),
            (b"EXIF".to_vec(), tiff(TiffSpec::le(7))),
        ]),
        "webp_exif_chunk_beyond" => {
            let mut chunk = Vec::new();
            chunk.extend_from_slice(b"EXIF");
            chunk.extend_from_slice(&0x7fff_ffffu32.to_le_bytes());
            chunk.extend_from_slice(&[0xff, 0x7f]);
            webp_with_chunks(vec![(b"EXIF".to_vec(), chunk)])
        }
        "webp_no_exif" => webp_with_chunks(vec![(b"VP8 ".to_vec(), vec![1, 2, 3])]),
        "png_no_exif" => vec![0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a],
        "empty" => Vec::new(),
        other => panic!("unknown fixture {other}"),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Parsed orientation values for every captured scenario.
#[test]
fn parses_the_same_orientation_values_as_the_upstream_parser() {
    let expected: &[(&str, u16)] = &[
        ("jpeg_le_1", 1),
        ("jpeg_le_2", 2),
        ("jpeg_le_3", 3),
        ("jpeg_le_4", 4),
        ("jpeg_le_5", 5),
        ("jpeg_le_6", 6),
        ("jpeg_le_7", 7),
        ("jpeg_le_8", 8),
        ("jpeg_be_6", 6),
        ("jpeg_be_2", 2),
        ("jpeg_le_8_extra_entry", 8),
        ("jpeg_truncated_tiff", 1),
        ("jpeg_ifd_beyond_len", 1),
        ("jpeg_entry_beyond_len", 4),
        ("jpeg_xmp_then_exif_6", 6),
        ("jpeg_app1_xmp_only", 1),
        ("jpeg_le_9", 1),
        ("jpeg_le_0", 1),
        ("jpeg_ff_run_before_app1_3", 3),
        ("jpeg_no_markers", 1),
        ("jpeg_app1_short", 1),
        ("webp_exif_prefix_6", 6),
        ("webp_exif_prefix_8", 8),
        ("webp_no_prefix_5", 5),
        ("webp_odd_padding_7", 7),
        ("webp_exif_chunk_beyond", 1),
        ("webp_no_exif", 1),
        ("png_no_exif", 1),
        ("empty", 1),
    ];
    for (name, value) in expected {
        assert_eq!(
            get_exif_orientation(&fixture(name)),
            *value,
            "orientation mismatch for {name}"
        );
    }
}

/// Pixel-level oracle: the transform matrix must permute the RGBA buffer
/// exactly like upstream applyExifOrientation did under real Photon.
#[test]
fn transform_matrix_matches_the_photon_oracle_pixels_byte_for_byte() {
    let oracle = oracle();
    let before = oracle["exif"]["jpeg_le_1"]["before"].as_str().unwrap();
    assert_eq!(hex(&grid_2x3()), before, "grid fixture drifted from oracle");
    for (name, entry) in oracle["exif"].as_object().unwrap() {
        let (mut width, mut height) = (2u32, 3u32);
        let mut rgba = grid_2x3();
        apply_orientation(&mut rgba, &mut width, &mut height, &fixture(name));
        assert_eq!(
            (width, height),
            (
                entry["width"].as_u64().unwrap() as u32,
                entry["height"].as_u64().unwrap() as u32
            ),
            "dimensions mismatch for {name}"
        );
        assert_eq!(
            hex(&rgba),
            entry["pixels"].as_str().unwrap(),
            "pixel mismatch for {name}"
        );
    }
}

#[test]
fn flips_are_in_place_and_rotations_swap_dimensions() {
    // Orientation 2..4 flip in place (upstream mutates the same image).
    let mut rgba = grid_2x3();
    let (mut w, mut h) = (2u32, 3u32);
    flip_h_in_place(&mut rgba, w, h);
    assert_eq!(
        hex(&rgba),
        oracle()["exif"]["jpeg_le_2"]["pixels"].as_str().unwrap()
    );
    flip_v_in_place(&mut rgba, w, h);
    assert_eq!(
        hex(&rgba),
        oracle()["exif"]["jpeg_le_3"]["pixels"].as_str().unwrap()
    );
    // Rotation: 3x2 output dims.
    let mut rgba = grid_2x3();
    apply_orientation(&mut rgba, &mut w, &mut h, &fixture("jpeg_le_6"));
    assert_eq!((w, h), (3, 2));
}
