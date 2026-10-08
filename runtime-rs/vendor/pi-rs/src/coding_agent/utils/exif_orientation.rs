//! Port of upstream `coding-agent/src/utils/exif-orientation.ts` (sha256
//! fbca93e04682…): pure EXIF orientation parsing for JPEG and WebP plus the
//! orientation transform over raw RGBA pixels.
//!
//! Every byte-layout branch (marker walk, TIFF endianness, IFD bounds checks,
//! RIFF chunk padding) mirrors upstream exactly; parse outcomes were captured
//! from the real upstream sources under node (see `tests/fixtures/image_oracle/`) and
//! are pinned by `exif_orientation_tests` against `image_oracle.json`.
//! Upstream `rotate90`/Photon `fliph`/`flipv` map to pure index permutations
//! over the RGBA buffer; these are byte-exact against the Photon WASM behavior
//! (verified against the captured oracle pixels).
//!
//! Difference from the previous decode path in
//! [`crate::coding_agent::utils::image_process`]: upstream ignores any
//! container-reported orientation and relies solely on this parser (PNG/GIF/
//! BMP therefore never rotate), so the decoder's `orientation()` is no longer
//! consulted.

/// Upstream `readOrientationFromTiff`.
pub fn read_orientation_from_tiff(bytes: &[u8], tiff_start: usize) -> u16 {
    if tiff_start + 8 > bytes.len() {
        return 1;
    }

    let byte_order = ((bytes[tiff_start] as u16) << 8) | u16::from(bytes[tiff_start + 1]);
    let le = byte_order == 0x4949;

    let read16 = |pos: usize| -> u16 {
        if pos + 2 > bytes.len() {
            return 0;
        }
        if le {
            u16::from(bytes[pos]) | (u16::from(bytes[pos + 1]) << 8)
        } else {
            (u16::from(bytes[pos]) << 8) | u16::from(bytes[pos + 1])
        }
    };

    let read32 = |pos: usize| -> i64 {
        if pos + 4 > bytes.len() {
            return 0;
        }
        if le {
            // Mirror the JS signed shift: the `<< 24` byte can sign-flip.
            i64::from(i32::from_le_bytes([
                bytes[pos],
                bytes[pos + 1],
                bytes[pos + 2],
                bytes[pos + 3],
            ]))
        } else {
            i64::from(u32::from_be_bytes([
                bytes[pos],
                bytes[pos + 1],
                bytes[pos + 2],
                bytes[pos + 3],
            ]))
        }
    };

    let ifd_offset = read32(tiff_start + 4);
    let Ok(tiff_start) = i64::try_from(tiff_start) else {
        return 1;
    };
    let ifd_start = tiff_start + ifd_offset;
    if ifd_start < 0 || ifd_start + 2 > bytes.len() as i64 {
        return 1;
    }
    let ifd_start = ifd_start as usize;

    let entry_count = read16(ifd_start);
    for i in 0..entry_count {
        let entry_pos = ifd_start + 2 + usize::from(i) * 12;
        if entry_pos + 12 > bytes.len() {
            return 1;
        }

        if read16(entry_pos) == 0x0112 {
            let value = read16(entry_pos + 8);
            return if (1..=8).contains(&value) { value } else { 1 };
        }
    }

    1
}

/// Upstream `findJpegTiffOffset`.
pub fn find_jpeg_tiff_offset(bytes: &[u8]) -> isize {
    let mut offset: i64 = 2;
    let len = bytes.len() as i64;
    while offset < len - 1 {
        let at = offset as usize;
        if bytes[at] != 0xff {
            return -1;
        }
        let marker = bytes[at + 1];
        if marker == 0xff {
            offset += 1;
            continue;
        }

        if marker == 0xe1 {
            if offset + 4 >= len {
                return -1;
            }
            let segment_start = at + 4;
            if segment_start + 6 > bytes.len() {
                return -1;
            }
            if has_exif_header(bytes, segment_start) {
                return (segment_start + 6) as isize;
            }
        }

        if offset + 4 > len {
            return -1;
        }
        let length = ((u16::from(bytes[at + 2]) << 8) | u16::from(bytes[at + 3])) as i64;
        offset += 2 + length;
    }

    -1
}

/// Upstream `findWebpTiffOffset`.
pub fn find_webp_tiff_offset(bytes: &[u8]) -> isize {
    let mut offset: i64 = 12;
    let len = bytes.len() as i64;
    while offset + 8 <= len {
        let at = offset as usize;
        let chunk_id = &bytes[at..at + 4];
        let chunk_size = i64::from(u32::from_le_bytes([
            bytes[at + 4],
            bytes[at + 5],
            bytes[at + 6],
            bytes[at + 7],
        ]));
        let data_start = offset + 8;

        if chunk_id == b"EXIF" {
            if data_start + chunk_size > len {
                return -1;
            }
            // Some WebP files have "Exif\0\0" prefix before the TIFF header.
            let tiff_start = if chunk_size >= 6 && has_exif_header(bytes, data_start as usize) {
                data_start + 6
            } else {
                data_start
            };
            return tiff_start as isize;
        }

        // RIFF chunks are padded to even size.
        offset = data_start + chunk_size + (chunk_size % 2);
    }

    -1
}

fn has_exif_header(bytes: &[u8], offset: usize) -> bool {
    offset + 6 <= bytes.len() && bytes[offset..offset + 6] == [0x45, 0x78, 0x69, 0x66, 0x00, 0x00]
}

/// Upstream `getExifOrientation`.
pub fn get_exif_orientation(bytes: &[u8]) -> u16 {
    // JPEG: starts with FF D8
    let tiff_offset = if bytes.len() >= 2 && bytes[0] == 0xff && bytes[1] == 0xd8 {
        find_jpeg_tiff_offset(bytes)
    }
    // WebP: starts with RIFF....WEBP
    else if bytes.len() >= 12
        && bytes[0..4] == [0x52, 0x49, 0x46, 0x46]
        && bytes[8..12] == [0x57, 0x45, 0x42, 0x50]
    {
        find_webp_tiff_offset(bytes)
    } else {
        return 1;
    };

    if tiff_offset < 0 {
        return 1;
    }
    read_orientation_from_tiff(bytes, tiff_offset as usize)
}

fn swap_pixels(rgba: &mut [u8], a: usize, b: usize) {
    for k in 0..4 {
        rgba.swap(a * 4 + k, b * 4 + k);
    }
}

/// Flip raw RGBA pixels horizontally in place (Photon `fliph`).
pub fn flip_h_in_place(rgba: &mut [u8], width: u32, height: u32) {
    let w = width as usize;
    for y in 0..height as usize {
        for x in 0..w / 2 {
            swap_pixels(rgba, y * w + x, y * w + (w - 1 - x));
        }
    }
}

/// Flip raw RGBA pixels vertically in place (Photon `flipv`).
pub fn flip_v_in_place(rgba: &mut [u8], width: u32, height: u32) {
    let w = width as usize;
    let h = height as usize;
    for y in 0..h / 2 {
        for x in 0..w {
            swap_pixels(rgba, y * w + x, (h - 1 - y) * w + x);
        }
    }
}

/// Upstream `rotate90`: rebuild the buffer with the caller's destination index
/// mapping; output dimensions are swapped (width = `h`, height = `w`).
fn rotate_90(
    src: &[u8],
    w: u32,
    h: u32,
    dst_index: impl Fn(usize, usize, usize, usize) -> usize,
) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let mut dst = vec![0u8; src.len()];
    for y in 0..h {
        for x in 0..w {
            let src_idx = (y * w + x) * 4;
            let dst_idx = dst_index(x, y, w, h) * 4;
            dst[dst_idx..dst_idx + 4].copy_from_slice(&src[src_idx..src_idx + 4]);
        }
    }
    dst
}

/// Upstream `applyExifOrientation` over a raw RGBA buffer. Flip orientations
/// mutate in place; rotations replace the buffer and swap the dimensions.
pub fn apply_orientation(
    rgba: &mut Vec<u8>,
    width: &mut u32,
    height: &mut u32,
    original_bytes: &[u8],
) {
    let orientation = get_exif_orientation(original_bytes);
    match orientation {
        1 => {}
        2 => flip_h_in_place(rgba, *width, *height),
        3 => {
            flip_h_in_place(rgba, *width, *height);
            flip_v_in_place(rgba, *width, *height);
        }
        4 => flip_v_in_place(rgba, *width, *height),
        5 => {
            let mut rotated = rotate_90(rgba, *width, *height, |x, y, _w, h| x * h + (h - 1 - y));
            flip_h_in_place(&mut rotated, *height, *width);
            *rgba = rotated;
            std::mem::swap(width, height);
        }
        6 => {
            *rgba = rotate_90(rgba, *width, *height, |x, y, _w, h| x * h + (h - 1 - y));
            std::mem::swap(width, height);
        }
        7 => {
            let mut rotated = rotate_90(rgba, *width, *height, |x, y, w, h| (w - 1 - x) * h + y);
            flip_h_in_place(&mut rotated, *height, *width);
            *rgba = rotated;
            std::mem::swap(width, height);
        }
        8 => {
            *rgba = rotate_90(rgba, *width, *height, |x, y, w, h| (w - 1 - x) * h + y);
            std::mem::swap(width, height);
        }
        _ => {}
    }
}

#[cfg(test)]
#[path = "exif_orientation_tests.rs"]
mod tests;
