//! QR 编码和 PNG 渲染在原生进程内完成；固定尺寸/边距沿用接入引导契约。
use super::{ChannelError, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use qrcode::{types::Color, EcLevel, QrCode};

pub(crate) fn data_url(text: &str, width: u32, margin: u32) -> Result<String> {
    if text.is_empty() {
        return Err(ChannelError::new("No input text"));
    }
    let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::M)
        .map_err(|error| ChannelError::new(error.to_string()))?;
    let modules = code.width() as u32;
    let cells = modules + 2 * margin;
    let width = width.max(cells);
    let mut image = image::RgbaImage::new(width, width);
    for y in 0..width {
        for x in 0..width {
            let module_x = x as u64 * cells as u64 / width as u64;
            let module_y = y as u64 * cells as u64 / width as u64;
            let dark = module_x >= margin as u64
                && module_y >= margin as u64
                && module_x < (margin + modules) as u64
                && module_y < (margin + modules) as u64
                && code[(
                    (module_x as u32 - margin) as usize,
                    (module_y as u32 - margin) as usize,
                )] == Color::Dark;
            image.put_pixel(
                x,
                y,
                image::Rgba(if dark {
                    [0, 0, 0, 255]
                } else {
                    [255, 255, 255, 255]
                }),
            );
        }
    }
    let mut bytes = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, image::ImageFormat::Png)
        .map_err(|error| ChannelError::new(error.to_string()))?;
    Ok(format!(
        "data:image/png;base64,{}",
        STANDARD.encode(bytes.into_inner())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_qr_has_real_modules_exact_dimensions_and_opaque_margin() {
        for width in [180, 248] {
            let data = data_url("https://t.me/BotFather", width, 2).unwrap();
            let image =
                image::load_from_memory(&STANDARD.decode(data.split_once(',').unwrap().1).unwrap())
                    .unwrap()
                    .to_rgba8();
            assert_eq!(image.dimensions(), (width, width));
            assert_eq!(image.get_pixel(0, 0).0, [255, 255, 255, 255]);
            assert!(image.pixels().all(|pixel| pixel.0[3] == 255));
            assert!(image.pixels().any(|pixel| pixel.0 == [0, 0, 0, 255]));
        }
    }
}
