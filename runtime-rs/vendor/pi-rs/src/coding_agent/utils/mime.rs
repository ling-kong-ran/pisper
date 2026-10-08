//! Port of upstream `coding-agent/src/utils/mime.ts`.
//!
//! Byte-level image mime sniffing for the mime types the product can attach.

const IMAGE_TYPE_SNIFF_BYTES: usize = 4100;
const PNG_SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

pub const MIME_JPEG: &str = "image/jpeg";
pub const MIME_PNG: &str = "image/png";
pub const MIME_GIF: &str = "image/gif";
pub const MIME_WEBP: &str = "image/webp";
pub const MIME_BMP: &str = "image/bmp";

/// Detect the supported image mime type from the leading bytes of a file.
/// Returns `None` for unsupported or unsafe-to-attach formats (e.g. JPEG
/// SPIFF, animated PNG).
pub fn detect_supported_image_mime_type(buffer: &[u8]) -> Option<&'static str> {
    if starts_with(buffer, &[0xff, 0xd8, 0xff]) {
        return if buffer.get(3) == Some(&0xf7) {
            None
        } else {
            Some(MIME_JPEG)
        };
    }
    if starts_with(buffer, &PNG_SIGNATURE) {
        return if is_png(buffer) && !is_animated_png(buffer) {
            Some(MIME_PNG)
        } else {
            None
        };
    }
    if starts_with_ascii(buffer, 0, b"GIF") {
        return Some(MIME_GIF);
    }
    if starts_with_ascii(buffer, 0, b"RIFF") && starts_with_ascii(buffer, 8, b"WEBP") {
        return Some(MIME_WEBP);
    }
    if starts_with_ascii(buffer, 0, b"BM") && is_bmp(buffer) {
        return Some(MIME_BMP);
    }
    None
}

/// Sniff the mime type of a file on disk (reads at most
/// `IMAGE_TYPE_SNIFF_BYTES` bytes from the start).
pub fn detect_supported_image_mime_type_from_file(
    file_path: &str,
) -> std::io::Result<Option<&'static str>> {
    use std::io::Read;

    let mut file = std::fs::File::open(file_path)?;
    let mut buffer = vec![0u8; IMAGE_TYPE_SNIFF_BYTES];
    let mut total = 0;
    while total < IMAGE_TYPE_SNIFF_BYTES {
        match file.read(&mut buffer[total..])? {
            0 => break,
            n => total += n,
        }
    }
    buffer.truncate(total);
    Ok(detect_supported_image_mime_type(&buffer))
}

fn is_png(buffer: &[u8]) -> bool {
    buffer.len() >= 16
        && read_uint32_be(buffer, PNG_SIGNATURE.len()) == 13
        && starts_with_ascii(buffer, 12, b"IHDR")
}

fn is_animated_png(buffer: &[u8]) -> bool {
    let mut offset = PNG_SIGNATURE.len();
    while offset + 8 <= buffer.len() {
        let chunk_length = read_uint32_be(buffer, offset);
        let chunk_type_offset = offset + 4;
        if starts_with_ascii(buffer, chunk_type_offset, b"acTL") {
            return true;
        }
        if starts_with_ascii(buffer, chunk_type_offset, b"IDAT") {
            return false;
        }

        let next_offset = offset + 8 + chunk_length as usize + 4;
        if next_offset <= offset || next_offset > buffer.len() {
            return false;
        }
        offset = next_offset;
    }
    false
}

fn is_bmp(buffer: &[u8]) -> bool {
    if buffer.len() < 26 {
        return false;
    }

    let declared_file_size = read_uint32_le(buffer, 2);
    let pixel_data_offset = read_uint32_le(buffer, 10);
    let dib_header_size = read_uint32_le(buffer, 14);
    if declared_file_size != 0 && declared_file_size < 26 {
        return false;
    }
    if pixel_data_offset < 14 + dib_header_size {
        return false;
    }
    if declared_file_size != 0 && pixel_data_offset >= declared_file_size {
        return false;
    }

    let (color_planes, bits_per_pixel);
    if dib_header_size == 12 {
        color_planes = read_uint16_le(buffer, 22);
        bits_per_pixel = read_uint16_le(buffer, 24);
    } else if (40..=124).contains(&dib_header_size) {
        if buffer.len() < 30 {
            return false;
        }
        color_planes = read_uint16_le(buffer, 26);
        bits_per_pixel = read_uint16_le(buffer, 28);
    } else {
        return false;
    }

    color_planes == 1 && [1, 4, 8, 16, 24, 32].contains(&bits_per_pixel)
}

fn read_uint16_le(buffer: &[u8], offset: usize) -> u32 {
    let lo = *buffer.get(offset).unwrap_or(&0) as u32;
    let hi = *buffer.get(offset + 1).unwrap_or(&0) as u32;
    lo + (hi << 8)
}

fn read_uint32_be(buffer: &[u8], offset: usize) -> u32 {
    let b0 = *buffer.get(offset).unwrap_or(&0) as u32;
    let b1 = *buffer.get(offset + 1).unwrap_or(&0) as u32;
    let b2 = *buffer.get(offset + 2).unwrap_or(&0) as u32;
    let b3 = *buffer.get(offset + 3).unwrap_or(&0) as u32;
    (b0 << 24) + (b1 << 16) + (b2 << 8) + b3
}

fn read_uint32_le(buffer: &[u8], offset: usize) -> u32 {
    let b0 = *buffer.get(offset).unwrap_or(&0) as u32;
    let b1 = *buffer.get(offset + 1).unwrap_or(&0) as u32;
    let b2 = *buffer.get(offset + 2).unwrap_or(&0) as u32;
    let b3 = *buffer.get(offset + 3).unwrap_or(&0) as u32;
    b0 + (b1 << 8) + (b2 << 16) + (b3 << 24)
}

fn starts_with(buffer: &[u8], bytes: &[u8]) -> bool {
    buffer.len() >= bytes.len() && buffer[..bytes.len()] == *bytes
}

fn starts_with_ascii(buffer: &[u8], offset: usize, text: &[u8]) -> bool {
    if buffer.len() < offset + text.len() {
        return false;
    }
    buffer[offset..offset + text.len()] == *text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::utils::oracle_data as oracle;

    fn u32be(n: u32) -> [u8; 4] {
        [(n >> 24) as u8, (n >> 16) as u8, (n >> 8) as u8, n as u8]
    }
    fn u32le(n: u32) -> [u8; 4] {
        [n as u8, (n >> 8) as u8, (n >> 16) as u8, (n >> 24) as u8]
    }
    fn ascii(text: &str) -> Vec<u8> {
        text.bytes().collect()
    }

    fn png_plain() -> Vec<u8> {
        let mut v = PNG_SIGNATURE.to_vec();
        v.extend(u32be(13));
        v.extend(ascii("IHDR"));
        v.extend(vec![0; 13]);
        v.extend(u32be(0));
        v.extend(ascii("IDAT"));
        v.extend([1, 2, 3, 4]);
        v.extend(u32be(0));
        v
    }

    fn png_animated() -> Vec<u8> {
        let mut v = PNG_SIGNATURE.to_vec();
        v.extend(u32be(13));
        v.extend(ascii("IHDR"));
        v.extend(vec![0; 13]);
        v.extend(u32be(0));
        v.extend(u32be(8));
        v.extend(ascii("acTL"));
        v.extend([1, 0, 0, 0]);
        v.extend(u32be(0));
        v
    }

    fn png_no_ihdr() -> Vec<u8> {
        let mut v = PNG_SIGNATURE.to_vec();
        v.extend(u32be(13));
        v.extend(ascii("IDAT"));
        v.extend(vec![0; 13]);
        v
    }

    fn bmp_1x1_24bpp() -> Vec<u8> {
        let mut b = vec![0u8; 58];
        b[0] = b'B';
        b[1] = b'M';
        b[2..6].copy_from_slice(&u32le(58));
        b[10..14].copy_from_slice(&u32le(54));
        b[14..18].copy_from_slice(&u32le(40));
        b[18..22].copy_from_slice(&u32le(1));
        b[22..26].copy_from_slice(&u32le(1));
        b[26..28].copy_from_slice(&u32le(1).to_vec()[..2]);
        b[28..30].copy_from_slice(&u32le(24).to_vec()[..2]);
        b[56] = 0xff;
        b
    }

    fn bmp_12_header() -> Vec<u8> {
        let mut b = vec![0u8; 26];
        b[0] = b'B';
        b[1] = b'M';
        b[2..6].copy_from_slice(&u32le(26));
        b[10..14].copy_from_slice(&u32le(26));
        b[14..18].copy_from_slice(&u32le(12));
        b[22..24].copy_from_slice(&u32le(1).to_vec()[..2]);
        b[24..26].copy_from_slice(&u32le(8).to_vec()[..2]);
        b
    }

    #[test]
    fn matches_upstream_sniffing_decisions() {
        let jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, b'J', b'F'];
        let jpeg_spiff = vec![0xff, 0xd8, 0xff, 0xf7, 0x00, 0x05];
        let webp = {
            let mut v = ascii("RIFF");
            v.extend(u32le(20));
            v.extend(ascii("WEBP"));
            v
        };
        let bmp = bmp_1x1_24bpp();
        let bmp12 = bmp_12_header();
        let mut bmp_bad_planes = bmp.clone();
        bmp_bad_planes[26..28].copy_from_slice(&u32le(2).to_vec()[..2]);
        let mut bmp_bad_bpp = bmp.clone();
        bmp_bad_bpp[28..30].copy_from_slice(&u32le(3).to_vec()[..2]);
        let mut bmp_bad_offset = bmp.clone();
        bmp_bad_offset[10..14].copy_from_slice(&u32le(20));
        let mut bmp_bad_size = bmp.clone();
        bmp_bad_size[2..6].copy_from_slice(&u32le(10));
        let bmp_short = bmp[..25].to_vec();

        let cases: Vec<(&str, Vec<u8>, &str)> = vec![
            ("jpeg", jpeg, MIME_JPEG),
            ("jpeg_spiff", jpeg_spiff, "null"),
            ("png", png_plain(), MIME_PNG),
            ("png_actl", png_animated(), "null"),
            ("png_no_ihdr", png_no_ihdr(), "null"),
            ("gif87", ascii("GIF87a"), MIME_GIF),
            ("gif89", ascii("GIF89a"), MIME_GIF),
            ("webp", webp, MIME_WEBP),
            ("bmp", bmp, MIME_BMP),
            ("bmp12", bmp12, "null"),
            ("bmp_bad_planes", bmp_bad_planes, "null"),
            ("bmp_bad_bpp", bmp_bad_bpp, "null"),
            ("bmp_bad_offset", bmp_bad_offset, "null"),
            ("bmp_bad_size", bmp_bad_size, "null"),
            ("bmp_short", bmp_short, "null"),
            ("empty", vec![], "null"),
            ("short_ffd8", vec![0xff, 0xd8], "null"),
            ("text", ascii("hello"), "null"),
        ];
        let mut matched = 0;
        for (name, bytes, expected) in &cases {
            let got = detect_supported_image_mime_type(bytes);
            let expected = if *expected == "null" {
                None
            } else {
                Some(*expected)
            };
            assert_eq!(got, expected, "case {name}");
            for (oracle_name, oracle_got) in oracle::DETECT_MIME {
                if *oracle_name == *name {
                    let oracle_expected = if *oracle_got == "null" {
                        None
                    } else {
                        Some(*oracle_got)
                    };
                    assert_eq!(got, oracle_expected, "oracle mismatch for {name}");
                    matched += 1;
                }
            }
        }
        assert_eq!(
            matched,
            oracle::DETECT_MIME.len(),
            "every oracle case covered"
        );
    }

    #[test]
    fn short_jpeg_header_matches_js_missing_index_without_panicking() {
        for (bytes, expected) in [
            (vec![], None),
            (vec![0xff], None),
            (vec![0xff, 0xd8], None),
            (vec![0xff, 0xd8, 0xff], Some(MIME_JPEG)),
            (vec![0xff, 0xd8, 0xff, 0xf7], None),
            (vec![0xff, 0xd8, 0xff, 0x00], Some(MIME_JPEG)),
        ] {
            assert_eq!(detect_supported_image_mime_type(&bytes), expected);
        }
    }

    #[test]
    fn sniffs_from_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tiny.bmp");
        std::fs::write(&path, bmp_1x1_24bpp()).expect("write");
        let detected =
            detect_supported_image_mime_type_from_file(path.to_str().expect("utf8")).expect("io");
        assert_eq!(detected, Some(MIME_BMP));

        let missing = detect_supported_image_mime_type_from_file("no-such-file-xyz.bmp");
        assert!(missing.is_err());
    }
}
