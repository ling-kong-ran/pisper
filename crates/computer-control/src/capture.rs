// 整屏截图。Windows 和 macOS 截主显示器，X11 截根窗口（多显示器时是拼起来的整个桌面）。
use crate::ScreenMap;
use base64::Engine;

/// BGRA，从左上角开始逐行，没有行间空隙。
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
    /// 这张图覆盖的屏幕范围，用输入 API 的坐标单位。
    pub screen_width: f64,
    pub screen_height: f64,
}

pub struct CapturedScreen {
    pub jpeg_base64: String,
    pub map: ScreenMap,
}

/// 缩小后再编码：模型按图片尺寸计费，1440 宽的文字也还看得清。
pub fn encode_jpeg(
    frame: &Frame,
    max_width: u32,
    quality: u8,
) -> Result<(Vec<u8>, u32, u32), String> {
    let expected = frame.width as usize * frame.height as usize * 4;
    if frame.width == 0 || frame.height == 0 || frame.bgra.len() < expected {
        return Err("The screen capture came back empty.".into());
    }
    let mut rgb = Vec::with_capacity(frame.width as usize * frame.height as usize * 3);
    for [blue, green, red, _] in frame.bgra[..expected].as_chunks::<4>().0 {
        rgb.extend_from_slice(&[*red, *green, *blue]);
    }
    let source = image::RgbImage::from_raw(frame.width, frame.height, rgb)
        .ok_or("The screen capture has the wrong size.")?;
    let resized = if frame.width > max_width && max_width > 0 {
        let height =
            (u64::from(frame.height) * u64::from(max_width) / u64::from(frame.width)).max(1) as u32;
        image::imageops::resize(
            &source,
            max_width,
            height,
            image::imageops::FilterType::Triangle,
        )
    } else {
        source
    };
    let (width, height) = resized.dimensions();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, quality)
        .encode_image(&resized)
        .map_err(|error| error.to_string())?;
    Ok((jpeg, width, height))
}

pub fn capture_screen(max_width: u32, quality: u8) -> Result<CapturedScreen, String> {
    let frame = platform::capture_frame()?;
    let (jpeg, width, height) = encode_jpeg(&frame, max_width, quality)?;
    Ok(CapturedScreen {
        jpeg_base64: base64::engine::general_purpose::STANDARD.encode(jpeg),
        map: ScreenMap {
            image_width: width,
            image_height: height,
            screen_width: frame.screen_width,
            screen_height: frame.screen_height,
        },
    })
}

#[cfg(target_os = "windows")]
mod platform {
    use super::Frame;
    use windows_sys::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
        SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, SRCCOPY,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

    pub fn capture_frame() -> Result<Frame, String> {
        // SAFETY: 句柄都在本函数内创建和释放；像素指针在 DIB 删除前读完。
        unsafe {
            let width = GetSystemMetrics(SM_CXSCREEN);
            let height = GetSystemMetrics(SM_CYSCREEN);
            if width <= 0 || height <= 0 {
                return Err("The screen size is not available.".into());
            }
            let screen = GetDC(std::ptr::null_mut());
            if screen.is_null() {
                return Err("The screen is not available (it may be locked).".into());
            }
            let memory = CreateCompatibleDC(screen);
            let mut info: BITMAPINFO = std::mem::zeroed();
            info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            info.bmiHeader.biWidth = width;
            // 负高度：从上到下，和 Frame 约定的顺序一致。
            info.bmiHeader.biHeight = -height;
            info.bmiHeader.biPlanes = 1;
            info.bmiHeader.biBitCount = 32;
            info.bmiHeader.biCompression = BI_RGB;
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(
                memory,
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            );
            let result = if bitmap.is_null() || bits.is_null() {
                Err("The screenshot buffer could not be created.".to_string())
            } else {
                let previous = SelectObject(memory, bitmap);
                // CAPTUREBLT 才带上分层窗口（很多现代应用的窗口都是）。
                let copied = BitBlt(
                    memory,
                    0,
                    0,
                    width,
                    height,
                    screen,
                    0,
                    0,
                    SRCCOPY | CAPTUREBLT,
                );
                SelectObject(memory, previous);
                if copied == 0 {
                    Err("The screen could not be captured (it may be locked or on a secure desktop).".to_string())
                } else {
                    let length = width as usize * height as usize * 4;
                    Ok(Frame {
                        width: width as u32,
                        height: height as u32,
                        bgra: std::slice::from_raw_parts(bits.cast::<u8>(), length).to_vec(),
                        screen_width: f64::from(width),
                        screen_height: f64::from(height),
                    })
                }
            };
            if !bitmap.is_null() {
                DeleteObject(bitmap);
            }
            DeleteDC(memory);
            ReleaseDC(std::ptr::null_mut(), screen);
            result
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::Frame;
    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGPoint {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGSize {
        width: f64,
        height: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGRect {
        origin: CGPoint,
        size: CGSize,
    }

    const ON_SCREEN_ONLY: u32 = 1;
    const NULL_WINDOW: u32 = 0;
    const IMAGE_DEFAULT: u32 = 0;
    // 32 位小端 + 预乘 alpha 在前，也就是内存里 BGRA。
    const BITMAP_BGRA: u32 = 2 | (2 << 12);

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGMainDisplayID() -> u32;
        fn CGDisplayBounds(display: u32) -> CGRect;
        // 14 起标记废弃，但仍可用；只有它不用强链接 ScreenCaptureKit（否则老系统启动不了）。
        fn CGWindowListCreateImage(
            bounds: CGRect,
            option: u32,
            window: u32,
            image: u32,
        ) -> *const c_void;
        fn CGImageGetWidth(image: *const c_void) -> usize;
        fn CGImageGetHeight(image: *const c_void) -> usize;
        fn CGImageRelease(image: *const c_void);
        fn CGColorSpaceCreateDeviceRGB() -> *const c_void;
        fn CGColorSpaceRelease(space: *const c_void);
        fn CGBitmapContextCreate(
            data: *mut c_void,
            width: usize,
            height: usize,
            bits_per_component: usize,
            bytes_per_row: usize,
            space: *const c_void,
            info: u32,
        ) -> *const c_void;
        fn CGContextRelease(context: *const c_void);
        fn CGContextDrawImage(context: *const c_void, rect: CGRect, image: *const c_void);
    }

    pub fn capture_frame() -> Result<Frame, String> {
        // SAFETY: CF 对象都在本函数内创建和释放；绘制时缓冲区大小与上下文一致。
        unsafe {
            let bounds = CGDisplayBounds(CGMainDisplayID());
            let image = CGWindowListCreateImage(bounds, ON_SCREEN_ONLY, NULL_WINDOW, IMAGE_DEFAULT);
            if image.is_null() {
                return Err(
                    "The screen could not be captured. Check the Screen Recording permission."
                        .into(),
                );
            }
            let width = CGImageGetWidth(image);
            let height = CGImageGetHeight(image);
            // 画进我们自己的 BGRA 缓冲区：截图本身的像素格式因显示器而异。
            let mut bgra = vec![0u8; width * height * 4];
            let space = CGColorSpaceCreateDeviceRGB();
            let context = CGBitmapContextCreate(
                bgra.as_mut_ptr().cast(),
                width,
                height,
                8,
                width * 4,
                space,
                BITMAP_BGRA,
            );
            CGColorSpaceRelease(space);
            if context.is_null() {
                CGImageRelease(image);
                return Err("The screenshot buffer could not be created.".into());
            }
            let rect = CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize {
                    width: width as f64,
                    height: height as f64,
                },
            };
            CGContextDrawImage(context, rect, image);
            CGContextRelease(context);
            CGImageRelease(image);
            Ok(Frame {
                width: width as u32,
                height: height as u32,
                bgra,
                // 输入坐标用点，截图是像素（Retina 下是两倍）。
                screen_width: bounds.size.width,
                screen_height: bounds.size.height,
            })
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::Frame;
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ConnectionExt, ImageFormat};

    pub fn capture_frame() -> Result<Frame, String> {
        let (connection, screen_number) =
            x11rb::connect(None).map_err(|error| error.to_string())?;
        let screen = &connection.setup().roots[screen_number];
        let (width, height) = (screen.width_in_pixels, screen.height_in_pixels);
        let reply = connection
            .get_image(ImageFormat::Z_PIXMAP, screen.root, 0, 0, width, height, !0)
            .map_err(|error| error.to_string())?
            .reply()
            .map_err(|error| error.to_string())?;
        if reply.depth != 24 && reply.depth != 32 {
            return Err(format!("A {}-bit screen is not supported.", reply.depth));
        }
        Ok(Frame {
            width: u32::from(width),
            height: u32::from(height),
            bgra: reply.data,
            screen_width: f64::from(width),
            screen_height: f64::from(height),
        })
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
mod platform {
    pub fn capture_frame() -> Result<super::Frame, String> {
        Err("Computer use is not supported on this platform.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u32, height: u32) -> Frame {
        Frame {
            width,
            height,
            bgra: vec![10; width as usize * height as usize * 4],
            screen_width: f64::from(width),
            screen_height: f64::from(height),
        }
    }

    #[test]
    fn large_screens_are_scaled_down_and_small_ones_are_not() {
        let (_, width, height) = encode_jpeg(&frame(3840, 2160), 1440, 70).unwrap();
        assert_eq!((width, height), (1440, 810));
        let (jpeg, width, height) = encode_jpeg(&frame(800, 600), 1440, 70).unwrap();
        assert_eq!((width, height), (800, 600));
        assert_eq!(&jpeg[..2], &[0xff, 0xd8]);
    }

    #[test]
    fn a_short_capture_is_an_error() {
        let mut short = frame(10, 10);
        short.bgra.truncate(100);
        assert!(encode_jpeg(&short, 1440, 70).is_err());
    }
}
