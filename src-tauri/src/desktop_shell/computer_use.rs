//! Computer Use 实时窗口流：按 CGWindowID 持续捕获目标窗口并推帧到前端活动卡片。
//!
//! 设计要点：
//! - macOS 用 `CGWindowListCreateImage`（单次同步调用，无异步 delegate）+ tokio 定时循环；
//!   帧编码为缩放后的 JPEG（逻辑像素坐标，与 AX/工具结果坐标系一致）。
//! - `CGWindowListCreateImage` 在 macOS 14+ 标记废弃但仍然可用；pi-computer-use 的单次
//!   截图走 SCScreenshotManager，这里是低频轮询（≤5fps），性能与兼容性可接受。
//!   未来如需高帧率可迁移到 SCStream delegate。
//! - 无屏幕录制权限时 macOS 返回空图：检测后经 Channel 下发引导事件而不是无限空帧。
//! - 帧走 Tauri IPC Channel（与终端输出同一模式），不占用 agent SSE 通道，
//!   也不需要额外的事件权限；TUI/移动端不受影响。
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::{ipc::Channel, State, WebviewWindow};

const MAX_STREAMS: usize = 4;
const MIN_FRAME_INTERVAL: Duration = Duration::from_millis(200);
const MAX_STREAM_LIFETIME: Duration = Duration::from_secs(15 * 60);
const MAX_FRAME_JPEG_BYTES: usize = 512 * 1024;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamStartInput {
    window_id: u32,
    #[serde(default = "default_max_width")]
    max_width: u32,
    #[serde(default = "default_fps")]
    fps: u32,
}

fn default_max_width() -> u32 {
    960
}

fn default_fps() -> u32 {
    3
}

#[derive(Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ComputerUseFrameEvent {
    Frame {
        window_id: u32,
        width: u32,
        height: u32,
        timestamp_ms: u64,
        /// JPEG 帧的 base64。帧经本地 IPC 传输，缩放后通常 <100KB。
        jpeg_base64: String,
    },
    /// 捕获失败（权限缺失/窗口关闭等）。流随后停止，前端应回退到静态预览。
    Error {
        code: String,
        message: String,
    },
    Stopped,
}

struct ManagedStream {
    stop: Arc<AtomicBoolLike>,
}

/// 用 Mutex<bool> 模拟跨线程停止标记，避免额外依赖。
struct AtomicBoolLike(Mutex<bool>);

impl AtomicBoolLike {
    fn new() -> Self {
        Self(Mutex::new(false))
    }
    fn stop(&self) {
        *self.0.lock().unwrap_or_else(|poison| poison.into_inner()) = true;
    }
    fn is_stopped(&self) -> bool {
        *self.0.lock().unwrap_or_else(|poison| poison.into_inner())
    }
}

#[derive(Default)]
pub struct ComputerUseStreamState(Arc<Mutex<HashMap<u32, ManagedStream>>>);

fn ensure_main(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("Computer Use streaming is available only in the main desktop window.".into())
    }
}

#[cfg(target_os = "macos")]
mod capture {
    use std::ffi::c_void;

    // CoreGraphics / CoreFoundation 的最小 FFI 面：按窗口捕获 + 读取像素数据。
    // 返回的 CGImage 为 32 位 BGRA（预乘 alpha），kCGWindowImageNominalResolution
    // 保证输出为逻辑像素（1x），与 AX 坐标系一致。
    type CGWindowID = u32;
    type CGImageRef = *const c_void;
    type CFDataRef = *const c_void;
    type CGDataProviderRef = *const c_void;

    #[repr(C)]
    #[derive(Copy, Clone)]
    pub struct CGPoint {
        pub x: f64,
        pub y: f64,
    }
    #[repr(C)]
    #[derive(Copy, Clone)]
    pub struct CGSize {
        pub width: f64,
        pub height: f64,
    }
    #[repr(C)]
    #[derive(Copy, Clone)]
    pub struct CGRect {
        pub origin: CGPoint,
        pub size: CGSize,
    }
    impl CGRect {
        pub const INFINITE: CGRect = CGRect {
            origin: CGPoint {
                x: f64::NEG_INFINITY,
                y: f64::NEG_INFINITY,
            },
            size: CGSize {
                width: f64::INFINITY,
                height: f64::INFINITY,
            },
        };
    }

    // 常量名保留 macOS SDK 原始命名（kCGWindowListOptionIncludingWindow 等）以便对照。
    #[allow(non_upper_case_globals)]
    const kCGWindowListOptionIncludingWindow: u32 = 1 << 3;
    #[allow(non_upper_case_globals)]
    const kCGWindowImageNominalResolution: u32 = 1 << 15;
    #[allow(non_upper_case_globals)]
    const kCGWindowImageBoundsIgnoreFraming: u32 = 1 << 0;

    extern "C" {
        fn CGWindowListCreateImage(
            rect: CGRect,
            list_option: u32,
            window_id: CGWindowID,
            image_option: u32,
        ) -> CGImageRef;
        fn CGImageGetWidth(image: CGImageRef) -> usize;
        fn CGImageGetHeight(image: CGImageRef) -> usize;
        fn CGImageGetBytesPerRow(image: CGImageRef) -> usize;
        fn CGImageGetBitsPerPixel(image: CGImageRef) -> usize;
        fn CGImageGetDataProvider(image: CGImageRef) -> CGDataProviderRef;
        fn CGDataProviderCopyData(provider: CGDataProviderRef) -> CFDataRef;
        fn CFDataGetBytePtr(data: CFDataRef) -> *const u8;
        fn CFDataGetLength(data: CFDataRef) -> isize;
        fn CFRelease(cf: *const c_void);
    }

    pub struct CapturedWindow {
        pub width: u32,
        pub height: u32,
        /// 紧凑的 BGRA 行（已按实际宽高重排，不含行尾对齐填充）。
        pub bgra: Vec<u8>,
    }

    /// 捕获失败原因：区分“窗口不存在”和“权限缺失”（后者返回空图）。
    pub enum CaptureError {
        WindowMissing,
        Denied,
        Failed(String),
    }

    /// 按窗口捕获一帧。窗口不存在时 CGWindowListCreateImage 返回 NULL；
    /// 无屏幕录制权限时通常返回 1x1 或全透明空图——用尺寸/内容判定。
    pub fn capture_window(window_id: u32) -> Result<CapturedWindow, CaptureError> {
        let options = kCGWindowImageNominalResolution | kCGWindowImageBoundsIgnoreFraming;
        // SAFETY: FFI 输入均为值类型；返回的 CGImageRef 按约定在读取后手动释放。
        let image = unsafe {
            CGWindowListCreateImage(
                CGRect::INFINITE,
                kCGWindowListOptionIncludingWindow,
                window_id,
                options,
            )
        };
        if image.is_null() {
            return Err(CaptureError::WindowMissing);
        }
        let (width, height, bytes_per_row, bits_per_pixel) = unsafe {
            (
                CGImageGetWidth(image),
                CGImageGetHeight(image),
                CGImageGetBytesPerRow(image),
                CGImageGetBitsPerPixel(image),
            )
        };
        if width == 0 || height == 0 || bits_per_pixel != 32 {
            unsafe { CFRelease(image) };
            return Err(CaptureError::Failed(
                "unsupported window image format".into(),
            ));
        }
        let result = unsafe {
            let provider = CGImageGetDataProvider(image);
            let data = if provider.is_null() {
                std::ptr::null()
            } else {
                CGDataProviderCopyData(provider)
            };
            if data.is_null() {
                CFRelease(image);
                return Err(CaptureError::Failed(
                    "window pixel data is unavailable".into(),
                ));
            }
            let length = CFDataGetLength(data) as usize;
            let byte_ptr = CFDataGetBytePtr(data);
            let bytes = if byte_ptr.is_null() || length < bytes_per_row * height {
                Vec::new()
            } else {
                std::slice::from_raw_parts(byte_ptr, length).to_vec()
            };
            CFRelease(data);
            (bytes, 0)
        };
        unsafe { CFRelease(image) };
        let (bytes, _) = result;
        if bytes.is_empty() {
            return Err(CaptureError::Failed(
                "window pixel data is unavailable".into(),
            ));
        }
        // 权限缺失的典型表现：1x1 空图或整帧全零/全不透明同色。1x1 视为拒绝；
        // 全零检测成本高，交给调用方的连续失败计数兜底。
        if width == 1 && height == 1 {
            return Err(CaptureError::Denied);
        }
        // 去掉每行行尾对齐填充，得到紧凑 BGRA。
        let pixel_bytes = 4;
        let mut bgra = Vec::with_capacity(width * height * pixel_bytes);
        for row in 0..height {
            let start = row * bytes_per_row;
            bgra.extend_from_slice(&bytes[start..start + width * pixel_bytes]);
        }
        Ok(CapturedWindow {
            width: width as u32,
            height: height as u32,
            bgra,
        })
    }

    /// BGRA → 缩放 JPEG。max_width 限制预览尺寸，节省 IPC 带宽与前端解码开销。
    pub fn encode_jpeg(
        bgra: &[u8],
        width: u32,
        height: u32,
        max_width: u32,
    ) -> Result<Vec<u8>, String> {
        let rgb = image::RgbaImage::from_fn(width, height, |x, y| {
            let offset = ((y as usize) * width as usize + x as usize) * 4;
            let b = bgra[offset];
            let g = bgra[offset + 1];
            let r = bgra[offset + 2];
            image::Rgba([r, g, b, 0xff])
        });
        let scale = if width > max_width && max_width > 0 {
            max_width as f64 / width as f64
        } else {
            1.0
        };
        let target_width = ((width as f64) * scale).round().max(1.0) as u32;
        let target_height = ((height as f64) * scale).round().max(1.0) as u32;
        let resized = if scale < 1.0 {
            image::imageops::resize(
                &rgb,
                target_width,
                target_height,
                image::imageops::FilterType::Triangle,
            )
        } else {
            rgb
        };
        let mut jpeg = Vec::new();
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 60);
        image::DynamicImage::ImageRgba8(resized)
            .write_with_encoder(encoder)
            .map_err(|error| format!("failed to encode jpeg: {error}"))?;
        Ok(jpeg)
    }

    /// BGRA → 缩放 JPEG → 经 Channel 下发。帧过大（例如超大窗口缩放失败）直接丢弃，
    /// 防止 IPC 载荷膨胀；编码错误向上冒泡由调用方终止流。
    pub fn encode_and_deliver(
        on_frame: &tauri::ipc::Channel<super::ComputerUseFrameEvent>,
        window_id: u32,
        captured: CapturedWindow,
        max_width: u32,
    ) -> Result<(), String> {
        let CapturedWindow {
            width,
            height,
            bgra,
        } = captured;
        let jpeg = encode_jpeg(&bgra, width, height, max_width)?;
        if jpeg.len() > super::MAX_FRAME_JPEG_BYTES {
            return Ok(());
        }
        use base64::Engine as _;
        let jpeg_base64 = base64::engine::general_purpose::STANDARD.encode(jpeg);
        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_millis() as u64)
            .unwrap_or(0);
        on_frame
            .send(super::ComputerUseFrameEvent::Frame {
                window_id,
                width,
                height,
                timestamp_ms,
                jpeg_base64,
            })
            .map_err(|error| format!("failed to deliver frame: {error}"))
    }

    /// 单元测试用的 BGRA 构造辅助：红/绿/蓝三色横条。
    #[cfg(test)]
    pub fn bgra_test_pattern(width: u32, height: u32) -> Vec<u8> {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let band = (y * 3 / height.max(1)).min(2);
                let (b, g, r) = match band {
                    0 => (0x20, 0x40, 0xd0),
                    1 => (0x40, 0xd0, 0x20),
                    _ => (0xd0, 0x20, 0x40),
                };
                let _ = x;
                pixels.extend_from_slice(&[b, g, r, 0xff]);
            }
        }
        pixels
    }
}

#[cfg(target_os = "macos")]
use capture::CaptureError;

#[tauri::command]
#[cfg(target_os = "macos")]
pub fn desktop_computer_use_start_stream(
    window: WebviewWindow,
    state: State<'_, ComputerUseStreamState>,
    input: StreamStartInput,
    on_frame: Channel<ComputerUseFrameEvent>,
) -> Result<(), String> {
    ensure_main(&window)?;
    let max_width = input.max_width.clamp(320, 1920);
    let fps = input.fps.clamp(1, 5);
    let interval = MIN_FRAME_INTERVAL.max(Duration::from_millis(1000 / fps as u64));
    let window_id = input.window_id;

    let mut streams = state
        .0
        .lock()
        .map_err(|_| "Computer Use stream state is unavailable.".to_string())?;
    if streams.contains_key(&window_id) {
        return Ok(());
    }
    if streams.len() >= MAX_STREAMS {
        return Err(format!(
            "No more than {MAX_STREAMS} window streams may run at once."
        ));
    }
    let stop = Arc::new(AtomicBoolLike::new());
    streams.insert(
        window_id,
        ManagedStream {
            stop: Arc::clone(&stop),
        },
    );
    drop(streams);

    let streams_state = Arc::clone(&state.0);
    tauri::async_runtime::spawn(async move {
        let started = Instant::now();
        let mut consecutive_failures = 0u32;
        loop {
            if stop.is_stopped() || started.elapsed() > MAX_STREAM_LIFETIME {
                break;
            }
            match capture::capture_window(window_id) {
                Ok(frame) => {
                    consecutive_failures = 0;
                    if let Err(message) =
                        capture::encode_and_deliver(&on_frame, window_id, frame, max_width)
                    {
                        let _ = on_frame.send(ComputerUseFrameEvent::Error {
                            code: "encode_failed".into(),
                            message,
                        });
                        break;
                    }
                }
                Err(CaptureError::WindowMissing) => {
                    let _ = on_frame.send(ComputerUseFrameEvent::Error {
                        code: "window_closed".into(),
                        message: "The target window is no longer available.".into(),
                    });
                    break;
                }
                Err(CaptureError::Denied) => {
                    let _ = on_frame.send(ComputerUseFrameEvent::Error {
                        code: "screen_recording_denied".into(),
                        message: "Screen Recording permission is required to mirror windows."
                            .into(),
                    });
                    break;
                }
                Err(CaptureError::Failed(message)) => {
                    // 偶发失败重试：连续 10 次失败才终止流。
                    consecutive_failures += 1;
                    if consecutive_failures >= 10 {
                        let _ = on_frame.send(ComputerUseFrameEvent::Error {
                            code: "capture_failed".into(),
                            message,
                        });
                        break;
                    }
                }
            }
            tokio::time::sleep(interval).await;
        }
        if let Ok(mut streams) = streams_state.lock() {
            streams.remove(&window_id);
        }
        let _ = on_frame.send(ComputerUseFrameEvent::Stopped);
    });
    Ok(())
}

#[tauri::command]
#[cfg(target_os = "macos")]
pub fn desktop_computer_use_stop_stream(
    window: WebviewWindow,
    state: State<'_, ComputerUseStreamState>,
    input: StreamStopInput,
) -> Result<(), String> {
    ensure_main(&window)?;
    if let Ok(streams) = state.0.lock() {
        if let Some(stream) = streams.get(&input.window_id) {
            stream.stop.stop();
        }
    }
    Ok(())
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamStopInput {
    window_id: u32,
}

// 非 macOS 桌面平台：保留命令面（前端统一调用路径），返回平台不支持错误。
#[cfg(not(target_os = "macos"))]
impl ComputerUseStreamState {
    fn unsupported() -> Result<(), String> {
        Err("Computer Use window streaming requires macOS in this build.".into())
    }
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub fn desktop_computer_use_start_stream(
    window: WebviewWindow,
    state: State<'_, ComputerUseStreamState>,
    input: StreamStartInput,
    on_frame: Channel<ComputerUseFrameEvent>,
) -> Result<(), String> {
    let _ = (window, state, input, on_frame);
    ComputerUseStreamState::unsupported()
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub fn desktop_computer_use_stop_stream(
    window: WebviewWindow,
    state: State<'_, ComputerUseStreamState>,
    input: StreamStopInput,
) -> Result<(), String> {
    let _ = (window, state, input);
    ComputerUseStreamState::unsupported()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn jpeg_encoding_downscales_and_produces_valid_output() {
        let width = 400u32;
        let height = 300u32;
        let bgra = capture::bgra_test_pattern(width, height);
        let jpeg = capture::encode_jpeg(&bgra, width, height, 200).expect("encode succeeds");
        assert!(!jpeg.is_empty());
        assert!(jpeg.len() < MAX_FRAME_JPEG_BYTES);
        // JPEG magic bytes。
        assert_eq!(&jpeg[0..2], &[0xff, 0xd8]);
        let decoded = image::load_from_memory(&jpeg).expect("decode succeeds");
        // 缩放目标宽度 200。
        assert_eq!(decoded.width(), 200);
        assert_eq!(decoded.height(), 150);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn jpeg_encoding_keeps_size_when_within_budget() {
        let width = 120u32;
        let height = 90u32;
        let bgra = capture::bgra_test_pattern(width, height);
        let jpeg = capture::encode_jpeg(&bgra, width, height, 960).expect("encode succeeds");
        let decoded = image::load_from_memory(&jpeg).expect("decode succeeds");
        assert_eq!(decoded.width(), width);
        assert_eq!(decoded.height(), height);
    }

    #[test]
    fn stream_start_input_defaults_and_clamps() {
        let input: StreamStartInput =
            serde_json::from_str(r#"{"windowId": 42}"#).expect("defaults apply");
        assert_eq!(input.max_width, default_max_width());
        assert_eq!(input.fps, default_fps());
    }
}
