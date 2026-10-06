//! Port of `src/tools/image.ts`: magic-byte sniffing for the image MIME types
//! the tool surface supports. Deliberately probes bytes, not file extensions.

const PNG_SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

/// `detectSupportedImageMimeType(buffer)` (`tools/image.ts`).
pub fn detect_supported_image_mime_type(bytes: &[u8]) -> Option<&'static str> {
    if starts_with(bytes, &[0xff, 0xd8, 0xff]) {
        return if bytes.get(3) == Some(&0xf7) {
            None
        } else {
            Some("image/jpeg")
        };
    }
    if starts_with(bytes, &PNG_SIGNATURE) {
        return if is_png(bytes) && !is_animated_png(bytes) {
            Some("image/png")
        } else {
            None
        };
    }
    if starts_with_ascii(bytes, 0, "GIF87a") || starts_with_ascii(bytes, 0, "GIF89a") {
        return Some("image/gif");
    }
    if starts_with_ascii(bytes, 0, "RIFF") && starts_with_ascii(bytes, 8, "WEBP") {
        return Some("image/webp");
    }
    if starts_with_ascii(bytes, 0, "BM") && is_bmp(bytes) {
        return Some("image/bmp");
    }
    None
}

/// `isPng(buffer)` (`tools/image.ts`).
fn is_png(buffer: &[u8]) -> bool {
    buffer.len() >= 16
        && read_u32_be(buffer, PNG_SIGNATURE.len()) == 13
        && starts_with_ascii(buffer, 12, "IHDR")
}

/// `isAnimatedPng(buffer)` (`tools/image.ts`).
fn is_animated_png(buffer: &[u8]) -> bool {
    let mut offset = PNG_SIGNATURE.len();
    while offset + 8 <= buffer.len() {
        let chunk_length = read_u32_be(buffer, offset);
        let chunk_type_offset = offset + 4;
        if starts_with_ascii(buffer, chunk_type_offset, "acTL") {
            return true;
        }
        if starts_with_ascii(buffer, chunk_type_offset, "IDAT") {
            return false;
        }
        let Some(next_offset) = (offset + 8)
            .checked_add(chunk_length as usize)
            .and_then(|value| value.checked_add(4))
        else {
            return false;
        };
        if next_offset <= offset || next_offset > buffer.len() {
            return false;
        }
        offset = next_offset;
    }
    false
}

/// `isBmp(buffer)` (`tools/image.ts`).
fn is_bmp(buffer: &[u8]) -> bool {
    if buffer.len() < 26 {
        return false;
    }
    let declared_file_size = read_u32_le(buffer, 2);
    let pixel_data_offset = read_u32_le(buffer, 10);
    let dib_header_size = read_u32_le(buffer, 14);
    if declared_file_size != 0 && declared_file_size < 26 {
        return false;
    }
    if (pixel_data_offset as u64) < 14 + dib_header_size as u64 {
        return false;
    }
    if declared_file_size != 0 && pixel_data_offset >= declared_file_size {
        return false;
    }

    let color_planes: u32;
    let bits_per_pixel: u32;
    if dib_header_size == 12 {
        color_planes = read_u16_le(buffer, 22);
        bits_per_pixel = read_u16_le(buffer, 24);
    } else if (40..=124).contains(&dib_header_size) {
        if buffer.len() < 30 {
            return false;
        }
        color_planes = read_u16_le(buffer, 26);
        bits_per_pixel = read_u16_le(buffer, 28);
    } else {
        return false;
    }
    color_planes == 1 && [1, 4, 8, 16, 24, 32].contains(&bits_per_pixel)
}

/// `readUint16LE(buffer, offset)` (`tools/image.ts`): out-of-range bytes read
/// as zero.
fn read_u16_le(buffer: &[u8], offset: usize) -> u32 {
    u32::from(buffer.get(offset).copied().unwrap_or(0))
        | u32::from(buffer.get(offset + 1).copied().unwrap_or(0)) << 8
}

/// `readUint32BE(buffer, offset)` (`tools/image.ts`).
fn read_u32_be(buffer: &[u8], offset: usize) -> u32 {
    (u32::from(buffer.get(offset).copied().unwrap_or(0)) << 24)
        | (u32::from(buffer.get(offset + 1).copied().unwrap_or(0)) << 16)
        | (u32::from(buffer.get(offset + 2).copied().unwrap_or(0)) << 8)
        | u32::from(buffer.get(offset + 3).copied().unwrap_or(0))
}

/// `readUint32LE(buffer, offset)` (`tools/image.ts`).
fn read_u32_le(buffer: &[u8], offset: usize) -> u32 {
    u32::from(buffer.get(offset).copied().unwrap_or(0))
        | (u32::from(buffer.get(offset + 1).copied().unwrap_or(0)) << 8)
        | (u32::from(buffer.get(offset + 2).copied().unwrap_or(0)) << 16)
        | (u32::from(buffer.get(offset + 3).copied().unwrap_or(0)) << 24)
}

/// `startsWith(buffer, bytes)` (`tools/image.ts`).
fn starts_with(buffer: &[u8], bytes: &[u8]) -> bool {
    buffer.len() >= bytes.len() && &buffer[..bytes.len()] == bytes
}

/// `startsWithAscii(buffer, offset, text)` (`tools/image.ts`).
fn starts_with_ascii(buffer: &[u8], offset: usize, text: &str) -> bool {
    let expected = text.as_bytes();
    buffer.len() >= offset + text.len() && buffer[offset..offset + text.len()] == expected[..]
}
