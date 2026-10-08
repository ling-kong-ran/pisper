//! Upstream tools/image.ts. Deliberately probes bytes, not file extensions.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
fn u32be(bytes: &[u8], at: usize) -> u32 {
    bytes
        .get(at..at + 4)
        .map(|s| u32::from_be_bytes(s.try_into().unwrap()))
        .unwrap_or(0)
}
fn u32le(bytes: &[u8], at: usize) -> u32 {
    bytes
        .get(at..at + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
        .unwrap_or(0)
}
fn u16le(bytes: &[u8], at: usize) -> u16 {
    bytes
        .get(at..at + 2)
        .map(|s| u16::from_le_bytes(s.try_into().unwrap()))
        .unwrap_or(0)
}
fn at(bytes: &[u8], offset: usize, signature: &[u8]) -> bool {
    bytes.get(offset..offset + signature.len()) == Some(signature)
}
fn animated_png(bytes: &[u8]) -> bool {
    let mut offset = PNG.len();
    while offset + 8 <= bytes.len() {
        if at(bytes, offset + 4, b"acTL") {
            return true;
        }
        if at(bytes, offset + 4, b"IDAT") {
            return false;
        }
        let Some(next) = offset
            .checked_add(u32be(bytes, offset) as usize)
            .and_then(|v| v.checked_add(12))
        else {
            return false;
        };
        if next <= offset || next > bytes.len() {
            return false;
        }
        offset = next;
    }
    false
}
fn is_bmp(bytes: &[u8]) -> bool {
    if bytes.len() < 26 {
        return false;
    }
    let size = u32le(bytes, 2) as u64;
    let pixel = u32le(bytes, 10) as u64;
    let dib = u32le(bytes, 14) as u64;
    if (size != 0 && size < 26) || pixel < 14 + dib || (size != 0 && pixel >= size) {
        return false;
    }
    let (planes, bits) = if dib == 12 {
        (u16le(bytes, 22), u16le(bytes, 24))
    } else if (40..=124).contains(&dib) && bytes.len() >= 30 {
        (u16le(bytes, 26), u16le(bytes, 28))
    } else {
        return false;
    };
    planes == 1 && [1, 4, 8, 16, 24, 32].contains(&bits)
}
pub fn detect_supported_image_mime_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return (bytes.get(3) != Some(&0xf7)).then_some("image/jpeg");
    }
    if bytes.starts_with(PNG) {
        return (bytes.len() >= 16
            && u32be(bytes, 8) == 13
            && at(bytes, 12, b"IHDR")
            && !animated_png(bytes))
        .then_some("image/png");
    }
    if bytes.starts_with(b"GIF") {
        return Some("image/gif");
    }
    if at(bytes, 0, b"RIFF") && at(bytes, 8, b"WEBP") {
        return Some("image/webp");
    }
    if bytes.starts_with(b"BM") && is_bmp(bytes) {
        return Some("image/bmp");
    }
    None
}
pub fn encode_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        output.push(ALPHABET[(a >> 2) as usize] as char);
        output.push(ALPHABET[(((a & 3) << 4) | (b >> 4)) as usize] as char);
        output.push(if chunk.len() < 2 {
            '='
        } else {
            ALPHABET[(((b & 15) << 2) | (c >> 6)) as usize] as char
        });
        output.push(if chunk.len() < 3 {
            '='
        } else {
            ALPHABET[(c & 63) as usize] as char
        });
    }
    output
}
