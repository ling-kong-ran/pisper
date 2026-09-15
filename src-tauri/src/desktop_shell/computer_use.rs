//! Computer Use 实时窗口流：按 CGWindowID 持续捕获目标窗口并推帧到前端活动卡片。
//!
//! 捕获优先走 ScreenCaptureKit SCStream（GPU 级、内容变化驱动、输出尺寸即缩放结果），
//! 默认 12fps、上限 60fps。为避免向主二进制写入 ScreenCaptureKit 的强链接（macOS
//! 12.3 之前不存在该框架，强链接会让旧系统上的整个应用无法启动），SCK 在运行时经
//! dlopen 加载，所有类与协议经 ObjC 运行时按名称解析（extern_protocol!/define_class!/
//! msg_send! 均不做链接期符号引用）；加载失败或系统过旧时回退到
//! CGWindowListCreateImage 定时轮询（能力降级但不中断）。帧编码统一为缩放 JPEG
//! （逻辑像素坐标，与 AX/工具结果坐标系一致）。
//!
//! 帧经 Tauri IPC Channel 下发（与终端输出同一模式），不占用 agent SSE 通道，
//! 也不需要额外的事件权限；TUI/移动端不受影响。
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};
use tauri::{ipc::Channel, State, WebviewWindow};

const MAX_STREAMS: usize = 4;
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

/// 默认 12fps：UI 动画观感接近实时，IPC 载荷与 CPU 占用可控；
/// 前端可按需请求最高 60fps（SCStream 路径内容不变时几乎不出帧）。
fn default_fps() -> u32 {
    12
}

const MAX_FPS: u32 = 60;
const POLLING_MAX_FPS: u32 = 5;
const POLLING_MIN_FRAME_INTERVAL: Duration = Duration::from_millis(200);

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
    /// 降级提示（非致命）：SCStream 不可用，已回退轮询模式。
    Downgraded {
        reason: String,
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

fn computer_use_channel_send(
    channel: &tauri::ipc::Channel<ComputerUseFrameEvent>,
    event: ComputerUseFrameEvent,
) {
    // Channel 下发失败（页面刷新/关闭）只意味着没人看帧，不构成错误路径。
    let _ = channel.send(event);
}

// ---------------------------------------------------------------------------
// 帧管道：生产者（SCStream 委托 / 轮询循环）→ 槽位（只留最新）→ 编码线程 → IPC
// ---------------------------------------------------------------------------

/// 一帧窗口捕获结果（紧凑 BGRA，无行尾对齐填充）。
pub struct CapturedWindow {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// “只留最新帧”的单槽邮箱：编码速度跟不上时丢弃旧帧，镜像始终显示最新画面。
struct FrameSlot {
    frame: Mutex<Option<CapturedWindow>>,
    ready: Condvar,
}

impl FrameSlot {
    fn new() -> Self {
        Self {
            frame: Mutex::new(None),
            ready: Condvar::new(),
        }
    }

    fn push(&self, frame: CapturedWindow) {
        let mut guard = self.frame.lock().unwrap_or_else(|p| p.into_inner());
        *guard = Some(frame);
        self.ready.notify_one();
    }

    /// 阻塞取最新帧；被停止时返回 None。
    fn take(&self, stop: &AtomicBoolLike) -> Option<CapturedWindow> {
        let mut guard = self.frame.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if let Some(frame) = guard.take() {
                return Some(frame);
            }
            if stop.is_stopped() {
                return None;
            }
            // 等待时释放锁；等待超时后重查停止标记，保证停止信号及时生效。
            let (next_guard, _timeout) = self
                .ready
                .wait_timeout(guard, Duration::from_millis(250))
                .unwrap_or_else(|p| p.into_inner());
            guard = next_guard;
        }
    }
}

/// 帧签名：抽样若干字节判断画面是否静止，静止帧跳过编码与下发，
/// 让静止窗口几乎不产生 CPU 与 IPC 开销（抽样远低于逐字节比较）。
fn frame_signature(frame: &CapturedWindow) -> u64 {
    const SAMPLES: usize = 64;
    let bytes = &frame.bgra;
    if bytes.is_empty() {
        return 0;
    }
    let stride = (bytes.len() / SAMPLES).max(1);
    let mut hash: u64 = 0xcbf29ce484222325;
    let mut offset = 0;
    while offset < bytes.len() {
        hash ^= bytes[offset] as u64;
        hash = hash.wrapping_mul(0x100000001b3);
        offset += stride;
    }
    // 尺寸参与签名，避免不同分辨率下抽样恰好碰撞。
    hash ^= ((frame.width as u64) << 32) | frame.height as u64;
    hash.wrapping_mul(0x100000001b3)
}

/// BGRA → 缩放 JPEG。max_width 限制预览尺寸，节省 IPC 带宽与前端解码开销。
fn encode_jpeg(bgra: &[u8], width: u32, height: u32, max_width: u32) -> Result<Vec<u8>, String> {
    let rgba = image::RgbaImage::from_fn(width, height, |x, y| {
        let offset = ((y as usize) * width as usize + x as usize) * 4;
        // BGRA 布局：低地址是 B，高地址是 A。
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
            &rgba,
            target_width,
            target_height,
            image::imageops::FilterType::Triangle,
        )
    } else {
        rgba
    };
    let mut jpeg = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 60);
    image::DynamicImage::ImageRgba8(resized)
        .write_with_encoder(encoder)
        .map_err(|error| format!("failed to encode jpeg: {error}"))?;
    Ok(jpeg)
}

/// 编码线程主体：取最新帧 → 静止跳过 → 编码 → Channel 下发。
/// 编码失败只停止本线程（帧管道关闭），由监督线程负责通知前端。
fn run_encoder(
    slot: Arc<FrameSlot>,
    stop: Arc<AtomicBoolLike>,
    channel: tauri::ipc::Channel<ComputerUseFrameEvent>,
    window_id: u32,
    max_width: u32,
) {
    let mut last_signature: Option<u64> = None;
    while let Some(frame) = slot.take(&stop) {
        let signature = frame_signature(&frame);
        // 静止帧跳过：SCK 空闲帧与轮询重复帧都在这里被过滤。
        if last_signature == Some(signature) {
            continue;
        }
        last_signature = Some(signature);
        let CapturedWindow {
            width,
            height,
            bgra,
        } = frame;
        let jpeg = match encode_jpeg(&bgra, width, height, max_width) {
            Ok(jpeg) => jpeg,
            Err(message) => {
                computer_use_channel_send(
                    &channel,
                    ComputerUseFrameEvent::Error {
                        code: "encode_failed".into(),
                        message,
                    },
                );
                stop.stop();
                continue;
            }
        };
        // 帧过大（异常窗口/缩放失效）直接丢弃，防止 IPC 载荷膨胀。
        if jpeg.len() > MAX_FRAME_JPEG_BYTES {
            continue;
        }
        use base64::Engine as _;
        let jpeg_base64 = base64::engine::general_purpose::STANDARD.encode(jpeg);
        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_millis() as u64)
            .unwrap_or(0);
        computer_use_channel_send(
            &channel,
            ComputerUseFrameEvent::Frame {
                window_id,
                width,
                height,
                timestamp_ms,
                jpeg_base64,
            },
        );
    }
}

// ---------------------------------------------------------------------------
// macOS 捕获实现：SCStream（dlopen + objc2 运行时绑定）主路径 + 轮询回退。
// ---------------------------------------------------------------------------
#[cfg(target_os = "macos")]
mod capture {
    /// 捕获失败原因：区分“窗口不存在”和“权限缺失”（后者返回空图）。
    pub enum CaptureError {
        WindowMissing,
        Denied,
        Failed(String),
    }

    // -- 回退路径：CGWindowListCreateImage 轮询（兼容与兜底） ------------------

    pub mod cg_ffi {
        use std::ffi::c_void;

        use super::super::CapturedWindow;
        use super::CaptureError;

        // CoreGraphics / CoreFoundation 的最小 FFI 面：按窗口捕获 + 读取像素数据。
        // CGWindowListCreateImage 在 macOS 14+ 标记废弃但仍然可用；仅作为 SCStream
        // 失败时的回退，低频轮询（≤5fps）性能可接受。
        type CGWindowID = u32;
        type CGImageRef = *const c_void;
        type CFDataRef = *const c_void;
        type CGDataProviderRef = *const c_void;

        #[repr(C)]
        #[derive(Copy, Clone)]
        struct CGPoint {
            x: f64,
            y: f64,
        }
        #[repr(C)]
        #[derive(Copy, Clone)]
        struct CGSize {
            width: f64,
            height: f64,
        }
        #[repr(C)]
        #[derive(Copy, Clone)]
        struct CGRect {
            origin: CGPoint,
            size: CGSize,
        }
        impl CGRect {
            const INFINITE: CGRect = CGRect {
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

        // 常量名保留 macOS SDK 原始命名以便对照。
        #[allow(non_upper_case_globals)]
        const kCGWindowListOptionIncludingWindow: u32 = 1 << 3;
        #[allow(non_upper_case_globals)]
        const kCGWindowImageNominalResolution: u32 = 1 << 15;
        #[allow(non_upper_case_globals)]
        const kCGWindowImageBoundsIgnoreFraming: u32 = 1 << 0;

        unsafe extern "C" {
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

        /// 去掉每行行尾对齐填充，得到紧凑 BGRA。
        fn compact_bgra(
            bytes: &[u8],
            width: usize,
            height: usize,
            bytes_per_row: usize,
        ) -> CapturedWindow {
            let mut bgra = Vec::with_capacity(width * height * 4);
            for row in 0..height {
                let start = row * bytes_per_row;
                bgra.extend_from_slice(&bytes[start..start + width * 4]);
            }
            CapturedWindow {
                width: width as u32,
                height: height as u32,
                bgra,
            }
        }

        /// 按窗口捕获一帧（回退路径）。窗口不存在时返回 NULL；
        /// 无屏幕录制权限时通常返回 1x1 或空图——用尺寸判定。
        pub fn capture_window_bgra(window_id: u32) -> Result<CapturedWindow, CaptureError> {
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
            let bytes = unsafe {
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
                bytes
            };
            unsafe { CFRelease(image) };
            if bytes.is_empty() {
                return Err(CaptureError::Failed(
                    "window pixel data is unavailable".into(),
                ));
            }
            // 权限缺失的典型表现：1x1 空图。
            if width == 1 && height == 1 {
                return Err(CaptureError::Denied);
            }
            Ok(compact_bgra(&bytes, width, height, bytes_per_row))
        }
    }

    // -- 主路径：ScreenCaptureKit SCStream（运行时 dlopen，无链接期依赖） ------
    //
    // 说明：不能使用 objc2-screen-capture-kit crate——它对 SCK 的 #[link] 属性会随
    // rlib 的 LC_LINKER_OPTION 强制写入主二进制（任何成员被引用即触发），旧 macOS
    // 上整个应用将无法启动。这里按 objc2 官方的 extern_protocol!/define_class!/
    // msg_send! 路径手工声明所需接口，类与协议全部按名称在运行时解析。

    pub mod sc_stream {
        use std::ffi::{c_char, c_int, c_void, CStr};
        use std::sync::mpsc;
        use std::sync::Arc;
        use std::time::Duration;

        use block2::RcBlock;
        use objc2::rc::{Allocated, Retained};
        use objc2::runtime::{AnyClass, AnyObject};
        use objc2::{define_class, extern_protocol, msg_send, AnyThread, DefinedClass};
        use objc2_core_foundation::CGRect;
        use objc2_core_media::{CMSampleBuffer, CMTime, CMTimeFlags};
        use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};

        use super::super::{CapturedWindow, FrameSlot};

        /// 监督线程需要持有 SCStream 句柄（以 AnyObject 形态）。
        pub type StreamHandle = Retained<AnyObject>;

        /// dlopen ScreenCaptureKit 并确认核心类已注册。
        /// 不引用任何 SCK 外部符号，保证主二进制在 macOS 12.3 之前仍能启动。
        pub fn ensure_screencapturekit_loaded() -> bool {
            static ONCE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            *ONCE.get_or_init(|| {
                let path =
                    c"/System/Library/Frameworks/ScreenCaptureKit.framework/ScreenCaptureKit";
                unsafe extern "C" {
                    fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
                }
                // SAFETY: 路径是静态字符串；RTLD_LAZY(1) 按需解析符号。
                let loaded = unsafe { !dlopen(path.as_ptr(), 1).is_null() };
                // dlopen 成功不代表类可用（防御异常系统状态），双重确认核心类。
                loaded
                    && any_class_available(b"SCStream\0")
                    && any_class_available(b"SCShareableContent\0")
                    && any_class_available(b"SCContentFilter\0")
                    && any_class_available(b"SCStreamConfiguration\0")
            })
        }

        fn any_class_available(name: &[u8]) -> bool {
            let name = CStr::from_bytes_with_nul(name).expect("name is NUL-terminated");
            AnyClass::get(name).is_some()
        }

        fn class_or_err(name: &CStr) -> Result<&'static AnyClass, String> {
            AnyClass::get(name)
                .ok_or_else(|| format!("class {} is unavailable", name.to_string_lossy()))
        }

        // SCK 帧输出协议。仅在运行时按名称解析，声明本身不产生链接期依赖。
        // SAFETY:
        // - 协议名称与 macOS SDK 的 SCStreamOutput 一致。
        // - SCStreamOutput 继承 NSObject 协议。
        // - 方法签名与 SDK 声明一致。
        extern_protocol!(
            #[allow(clippy::missing_safety_doc)]
            unsafe trait SCStreamOutput: NSObjectProtocol {
                #[optional]
                #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
                #[allow(non_snake_case)]
                fn stream_didOutputSampleBuffer_ofType(
                    &self,
                    stream: &AnyObject,
                    sample_buffer: &CMSampleBuffer,
                    of_type: isize,
                );
            }
        );

        /// 接收 SCK 帧回调的自定义类：把样本缓冲区像素拷贝进槽位。
        /// 类经 define_class! 注册一次，每个流持有独立槽位实例。
        struct SinkIvars {
            slot: Arc<FrameSlot>,
        }

        define_class!(
            // SAFETY:
            // - NSObject 无子类化要求。
            // - CuFrameSink 未实现 Drop。
            #[unsafe(super(NSObject))]
            #[ivars = SinkIvars]
            struct CuFrameSink;

            unsafe impl NSObjectProtocol for CuFrameSink {}

            unsafe impl SCStreamOutput for CuFrameSink {
                // 方法名与 SDK 协议声明保持一致。
                #[allow(non_snake_case)]
                #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
                fn stream_didOutputSampleBuffer_ofType(
                    &self,
                    _stream: &AnyObject,
                    sample_buffer: &CMSampleBuffer,
                    _of_type: isize,
                ) {
                    self.on_sample(sample_buffer)
                }
            }
        );

        impl CuFrameSink {
            fn new(slot: Arc<FrameSlot>) -> Retained<Self> {
                let this = Self::alloc().set_ivars(SinkIvars { slot });
                // SAFETY: 超类 init 不改变对象身份，返回值即本实例。
                unsafe { msg_send![super(this), init] }
            }

            /// 样本缓冲 → 锁定像素 → 紧凑 BGRA 拷贝 → 槽位。
            /// 回调发生在 SCK 的派发队列上，只做拷贝不编码，避免反压捕获管线。
            fn on_sample(&self, sample_buffer: &CMSampleBuffer) {
                let image = unsafe { sample_buffer.image_buffer() };
                let Some(image) = image else { return };
                // 防御性类型校验：SCK 屏幕帧的 image buffer 应为 CVPixelBuffer，
                // 但不做类型确认就直接按像素缓冲读取会有未定义行为风险。
                // SAFETY: CoreFoundation 的类型 ID 是稳定 C 接口；指针源自保留的
                // CFType 对象，校验通过前不做任何字段访问。
                let is_pixel_buffer = unsafe {
                    extern "C" {
                        fn CFGetTypeID(cf: *const std::ffi::c_void) -> usize;
                    }
                    let obj: &objc2_core_foundation::CFType = &image;
                    let obj = std::ptr::from_ref::<objc2_core_foundation::CFType>(obj)
                        as *const std::ffi::c_void;
                    CFGetTypeID(obj) == objc2_core_video::CVPixelBufferGetTypeID()
                };
                if !is_pixel_buffer {
                    return;
                }
                // 两个 extern_class 结构在内存布局上等价（均为对象指针的透明包装），
                // 类型校验通过后指针重解释安全。
                let pixel_buffer: &objc2_core_video::CVPixelBuffer =
                    unsafe { &*(&*image as *const _ as *const _) };
                let flags = objc2_core_video::CVPixelBufferLockFlags(0);
                if unsafe { objc2_core_video::CVPixelBufferLockBaseAddress(pixel_buffer, flags) }
                    != 0
                {
                    return;
                }
                let width = objc2_core_video::CVPixelBufferGetWidth(pixel_buffer);
                let height = objc2_core_video::CVPixelBufferGetHeight(pixel_buffer);
                let bytes_per_row = objc2_core_video::CVPixelBufferGetBytesPerRow(pixel_buffer);
                let base = objc2_core_video::CVPixelBufferGetBaseAddress(pixel_buffer) as *const u8;
                let mut compact = None;
                if width > 0 && height > 0 && bytes_per_row >= width * 4 && !base.is_null() {
                    // SAFETY: base 在 lock 后有效；按行拷贝后立即解锁。
                    let bytes = unsafe { std::slice::from_raw_parts(base, bytes_per_row * height) };
                    let mut bgra = Vec::with_capacity(width * height * 4);
                    for row in 0..height {
                        let start = row * bytes_per_row;
                        bgra.extend_from_slice(&bytes[start..start + width * 4]);
                    }
                    compact = Some(CapturedWindow {
                        width: width as u32,
                        height: height as u32,
                        bgra,
                    });
                }
                unsafe { objc2_core_video::CVPixelBufferUnlockBaseAddress(pixel_buffer, flags) };
                if let Some(frame) = compact {
                    self.ivars().slot.push(frame);
                }
            }
        }

        /// 枚举共享内容并按窗口 ID 匹配窗口对象，附带窗口尺寸（逻辑点）。
        fn find_sc_window(
            window_id: u32,
        ) -> Result<Option<(Retained<AnyObject>, f64, f64)>, String> {
            let shareable_content_class = class_or_err(c"SCShareableContent")?;
            let (tx, rx) = mpsc::channel();
            let block = RcBlock::new(move |content: *mut AnyObject, error: *mut NSError| {
                // SAFETY: 回调指针非空时由 SCK 保证有效；retain 转移所有权。
                let content =
                    unsafe { (!content.is_null()).then(|| Retained::retain(content)) }.flatten();
                let error =
                    unsafe { (!error.is_null()).then(|| Retained::retain(error)) }.flatten();
                let _ = tx.send((content, error));
            });
            // SAFETY: 类已随 dlopen 注册并确认；块签名与协议声明一致。
            // onScreenWindowsOnly=false：被最小化/移出屏幕的目标窗口仍可镜像
            // （desktopIndependentWindow 过滤器不依赖窗口在屏状态）。
            unsafe {
                let _: () = msg_send![
                    shareable_content_class,
                    getShareableContentExcludingDesktopWindows: false,
                    onScreenWindowsOnly: false,
                    completionHandler: &*block
                ];
            }
            let (content, error) = rx
                .recv_timeout(Duration::from_secs(10))
                .map_err(|_| "timed out while enumerating shareable content".to_string())?;
            if let Some(error) = error {
                let message = error.localizedDescription().to_string();
                return Err(format!("shareable content is unavailable: {message}"));
            }
            let Some(content) = content else {
                return Err("shareable content is unavailable".into());
            };
            // SAFETY: windows getter 返回 NSArray<SCWindow>；元素类型不影响
            // windowID/frame 消息调用，按 AnyObject 处理。
            let windows: Option<Retained<NSArray<AnyObject>>> =
                unsafe { msg_send![&*content, windows] };
            let Some(windows) = windows else {
                return Err("shareable content has no windows".into());
            };
            for window in windows.to_vec() {
                // SAFETY: SCWindow 响应 windowID（返回 CGWindowID=u32）与 frame。
                let candidate: u32 = unsafe { msg_send![&*window, windowID] };
                if candidate == window_id {
                    // SAFETY: SCWindow 响应 frame（返回 CGRect）。
                    let frame: CGRect = unsafe { msg_send![&*window, frame] };
                    return Ok(Some((window, frame.size.width, frame.size.height)));
                }
            }
            Ok(None)
        }

        /// SCStream 停止辅助：等待停止完成（超时即视为流已结束）。
        fn stop_capture_and_wait(stream: &AnyObject) {
            let (tx, rx) = mpsc::channel();
            let block = RcBlock::new(move |_error: *mut NSError| {
                let _ = tx.send(());
            });
            // SAFETY: 停止回调可能携带错误指针，忽略内容即可。
            unsafe {
                let _: () = msg_send![
                    stream,
                    stopCaptureWithCompletionHandler: &*block
                ];
            }
            let _ = rx.recv_timeout(Duration::from_secs(2));
        }

        /// 启动 SCStream 主路径。返回的句柄负责停流；槽位接收最新帧。
        pub fn start(
            window_id: u32,
            max_width: u32,
            fps: u32,
            slot: Arc<FrameSlot>,
        ) -> Result<StreamHandle, String> {
            let Some((window, window_width, window_height)) = find_sc_window(window_id)? else {
                return Err("window is not available for capture".into());
            };
            let scale = if window_width > max_width as f64 && max_width > 0 {
                max_width as f64 / window_width
            } else {
                1.0
            };
            let target_width = ((window_width * scale).round() as usize).max(1);
            let target_height = ((window_height * scale).round() as usize).max(1);
            let timescale = fps.clamp(1, super::super::MAX_FPS) as i32;

            let filter_class = class_or_err(c"SCContentFilter")?;
            let config_class = class_or_err(c"SCStreamConfiguration")?;
            let stream_class = class_or_err(c"SCStream")?;

            // SAFETY: 以下 SCK 调用均发生在 dlopen 且核心类确认之后，
            // 选择器与 SDK 声明一致，参数类型满足 ObjC 编码要求。
            unsafe {
                // alloc + initWithDesktopIndependentWindow: 得到单窗口过滤器。
                let filter: Retained<AnyObject> = {
                    let alloc: Allocated<AnyObject> = msg_send![filter_class, alloc];
                    msg_send![alloc, initWithDesktopIndependentWindow: &*window]
                };
                let config: Retained<AnyObject> = msg_send![config_class, new];
                let _: () = msg_send![&*config, setWidth: target_width];
                let _: () = msg_send![&*config, setHeight: target_height];
                // kCVPixelFormatType_32BGRA 的 fourcc 'BGRA'。
                let _: () = msg_send![&*config, setPixelFormat: u32::from_be_bytes(*b"BGRA")];
                let _: () = msg_send![&*config, setShowsCursor: false];
                let _: () = msg_send![&*config, setScalesToFit: true];
                let _: () = msg_send![&*config, setQueueDepth: 3];
                let _: () = msg_send![
                    &*config,
                    setMinimumFrameInterval: CMTime {
                        value: 1,
                        timescale,
                        flags: CMTimeFlags(1), // kCMTimeFlags_Valid
                        epoch: 0,
                    },
                ];

                let stream: Retained<AnyObject> = {
                    let alloc: Allocated<AnyObject> = msg_send![stream_class, alloc];
                    msg_send![
                        alloc,
                        initWithFilter: &*filter,
                        configuration: &*config,
                        delegate: std::ptr::null::<AnyObject>(),
                    ]
                };
                let sink = CuFrameSink::new(slot);
                let queue = dispatch2::DispatchQueue::new("com.pisper.computer-use", None);
                let mut error: *mut NSError = std::ptr::null_mut();
                let ok: bool = msg_send![
                    &*stream,
                    addStreamOutput: &*sink,
                    // SCStreamOutputTypeScreen = 0
                    type: 0,
                    sampleHandlerQueue: AsRef::<AnyObject>::as_ref(&queue),
                    error: &mut error,
                ];
                if !ok {
                    let message = if error.is_null() {
                        "unknown error".to_string()
                    } else {
                        let error = Retained::retain(error).expect("non-null error");
                        error.localizedDescription().to_string()
                    };
                    return Err(format!("failed to attach capture output: {message}"));
                }

                let (started_tx, started_rx) = mpsc::channel();
                let start_block = RcBlock::new(move |error: *mut NSError| {
                    let error = (!error.is_null())
                        .then(|| Retained::retain(error))
                        .flatten();
                    let _ = started_tx.send(error);
                });
                let _: () = msg_send![&*stream, startCaptureWithCompletionHandler: &*start_block,];
                match started_rx.recv_timeout(Duration::from_secs(10)) {
                    Ok(None) => Ok(stream),
                    Ok(Some(error)) => Err(format!(
                        "failed to start capture: {}",
                        error.localizedDescription()
                    )),
                    Err(_) => Err("timed out while starting capture".into()),
                }
            }
        }

        pub fn stop(stream: &StreamHandle) {
            stop_capture_and_wait(stream);
        }
    }
}

#[cfg(target_os = "macos")]
use capture::{sc_stream, CaptureError};

#[cfg(target_os = "macos")]
#[tauri::command]
pub fn desktop_computer_use_start_stream(
    window: WebviewWindow,
    state: State<'_, ComputerUseStreamState>,
    input: StreamStartInput,
    on_frame: Channel<ComputerUseFrameEvent>,
) -> Result<(), String> {
    ensure_main(&window)?;
    let max_width = input.max_width.clamp(320, 1920);
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

    // 每条流一个监督线程：负责启动 SCStream/轮询、守护生命周期并收尾。
    // SCShareableContent 枚举与启动均为阻塞等待（最长 10s），不能占用异步运行时。
    // state 是借用句柄，把内部 Arc 移交给监督线程以便收尾时清理注册表。
    let streams_state = Arc::clone(&state.0);
    let rollback_state = Arc::clone(&state.0);
    let supervisor = std::thread::Builder::new()
        .name(format!("cu-stream-{window_id}"))
        .spawn(move || {
            let started = Instant::now();
            let slot = Arc::new(FrameSlot::new());
            let encoder_stop = Arc::new(AtomicBoolLike::new());
            let mut sc_stream: Option<sc_stream::StreamHandle> = None;

            // 主路径：SCStream。句柄由监督线程持有，退出时统一停流。
            if sc_stream::ensure_screencapturekit_loaded() {
                match sc_stream::start(window_id, max_width, input.fps, Arc::clone(&slot)) {
                    Ok(handle) => sc_stream = Some(handle),
                    Err(message) => {
                        // SCStream 不可用不立即失败：先降级到轮询，前端仍能看到画面。
                        eprintln!(
                            "[computer-use] SCStream unavailable, falling back to polling: {message}"
                        );
                        computer_use_channel_send(
                            &on_frame,
                            ComputerUseFrameEvent::Downgraded { reason: message },
                        );
                    }
                }
            } else {
                computer_use_channel_send(
                    &on_frame,
                    ComputerUseFrameEvent::Downgraded {
                        reason: "ScreenCaptureKit is unavailable on this system".into(),
                    },
                );
            }

            // 编码线程：槽位里的最新帧 → JPEG → IPC。
            let encoder = std::thread::Builder::new()
                .name(format!("cu-encode-{window_id}"))
                .spawn({
                    let slot = Arc::clone(&slot);
                    let encoder_stop = Arc::clone(&encoder_stop);
                    let channel = on_frame.clone();
                    move || run_encoder(slot, encoder_stop, channel, window_id, max_width)
                });

            if sc_stream.is_some() {
                // SCStream 路径只做生命周期看护：停止信号/超时时停流。
                while !stop.is_stopped() && started.elapsed() <= MAX_STREAM_LIFETIME {
                    std::thread::sleep(Duration::from_millis(500));
                }
            } else {
                // 轮询回退路径：按目标节奏捕获并推入槽位。
                let fps = input.fps.clamp(1, POLLING_MAX_FPS);
                let interval =
                    POLLING_MIN_FRAME_INTERVAL.max(Duration::from_millis(1000 / fps as u64));
                let mut consecutive_failures = 0u32;
                while !stop.is_stopped() && started.elapsed() <= MAX_STREAM_LIFETIME {
                    match capture::cg_ffi::capture_window_bgra(window_id) {
                        Ok(frame) => {
                            consecutive_failures = 0;
                            slot.push(frame);
                        }
                        Err(CaptureError::WindowMissing) => {
                            computer_use_channel_send(
                                &on_frame,
                                ComputerUseFrameEvent::Error {
                                    code: "window_closed".into(),
                                    message: "The target window is no longer available.".into(),
                                },
                            );
                            break;
                        }
                        Err(CaptureError::Denied) => {
                            computer_use_channel_send(
                                &on_frame,
                                ComputerUseFrameEvent::Error {
                                    code: "screen_recording_denied".into(),
                                    message: "Screen Recording permission is required to mirror windows.".into(),
                                },
                            );
                            break;
                        }
                        Err(CaptureError::Failed(message)) => {
                            // 偶发失败重试：连续 10 次失败才终止流。
                            consecutive_failures += 1;
                            if consecutive_failures >= 10 {
                                computer_use_channel_send(
                                    &on_frame,
                                    ComputerUseFrameEvent::Error {
                                        code: "capture_failed".into(),
                                        message,
                                    },
                                );
                                break;
                            }
                        }
                    }
                    std::thread::sleep(interval);
                }
            }

            // 收尾：停 SCStream、停编码线程、清理注册表并通知前端。
            if let Some(stream) = sc_stream.as_ref() {
                sc_stream::stop(stream);
            }
            encoder_stop.stop();
            if let Ok(encoder) = encoder {
                let _ = encoder.join();
            }
            if let Ok(mut streams) = streams_state.lock() {
                streams.remove(&window_id);
            }
            computer_use_channel_send(&on_frame, ComputerUseFrameEvent::Stopped);
        });
    if supervisor.is_err() {
        // 监督线程创建失败：立即回滚注册表并报错。
        if let Ok(mut streams) = rollback_state.lock() {
            streams.remove(&window_id);
        }
        return Err("failed to start Computer Use stream supervisor.".into());
    }
    Ok(())
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamStopInput {
    window_id: u32,
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

// 非 macOS 桌面平台：保留命令面（前端统一调用路径），返回平台不支持错误。
#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub fn desktop_computer_use_start_stream(
    window: WebviewWindow,
    state: State<'_, ComputerUseStreamState>,
    input: StreamStartInput,
    on_frame: Channel<ComputerUseFrameEvent>,
) -> Result<(), String> {
    let _ = (window, state, input, on_frame);
    Err("Computer Use window streaming requires macOS in this build.".into())
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub fn desktop_computer_use_stop_stream(
    window: WebviewWindow,
    state: State<'_, ComputerUseStreamState>,
    input: StreamStopInput,
) -> Result<(), String> {
    let _ = (window, state, input);
    Err("Computer Use window streaming requires macOS in this build.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn jpeg_encoding_downscales_and_produces_valid_output() {
        let width = 400u32;
        let height = 300u32;
        let bgra = bgra_test_pattern(width, height);
        let jpeg = encode_jpeg(&bgra, width, height, 200).expect("encode succeeds");
        assert!(!jpeg.is_empty());
        assert!(jpeg.len() < MAX_FRAME_JPEG_BYTES);
        // JPEG magic bytes。
        assert_eq!(&jpeg[0..2], &[0xff, 0xd8]);
        let decoded = image::load_from_memory(&jpeg).expect("decode succeeds");
        assert_eq!(decoded.width(), 200);
        assert_eq!(decoded.height(), 150);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn jpeg_encoding_keeps_size_when_within_budget() {
        let width = 120u32;
        let height = 90u32;
        let bgra = bgra_test_pattern(width, height);
        let jpeg = encode_jpeg(&bgra, width, height, 960).expect("encode succeeds");
        let decoded = image::load_from_memory(&jpeg).expect("decode succeeds");
        assert_eq!(decoded.width(), width);
        assert_eq!(decoded.height(), height);
    }

    #[test]
    fn frame_signature_distinguishes_changes() {
        let a = CapturedWindow {
            width: 4,
            height: 4,
            bgra: bgra_test_pattern(4, 4),
        };
        let mut b = CapturedWindow {
            width: 4,
            height: 4,
            bgra: bgra_test_pattern(4, 4),
        };
        assert_eq!(frame_signature(&a), frame_signature(&b));
        let last = b.bgra.len() - 1;
        b.bgra[0] ^= 0xff;
        b.bgra[last] ^= 0xff;
        assert_ne!(frame_signature(&a), frame_signature(&b));
        // 尺寸变化也须区分。
        let c = CapturedWindow {
            width: 2,
            height: 8,
            bgra: bgra_test_pattern(4, 4),
        };
        assert_ne!(frame_signature(&a), frame_signature(&c));
    }

    #[test]
    fn frame_slot_keeps_latest_frame() {
        let slot = FrameSlot::new();
        let stop = AtomicBoolLike::new();
        slot.push(CapturedWindow {
            width: 1,
            height: 1,
            bgra: vec![1, 2, 3, 4],
        });
        slot.push(CapturedWindow {
            width: 2,
            height: 2,
            bgra: vec![9; 16],
        });
        let frame = slot.take(&stop).expect("frame is available");
        // 后推入的帧覆盖先推入的帧。
        assert_eq!(frame.width, 2);
        stop.stop();
        assert!(slot.take(&stop).is_none());
    }

    #[test]
    fn stream_start_input_defaults() {
        let input: StreamStartInput =
            serde_json::from_str(r#"{"windowId": 42}"#).expect("defaults apply");
        assert_eq!(input.max_width, default_max_width());
        assert_eq!(input.fps, default_fps());
    }

    /// 红/绿/蓝三色横条测试图（BGRA 紧凑布局）。
    fn bgra_test_pattern(width: u32, height: u32) -> Vec<u8> {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for _x in 0..width {
                let band = (y * 3 / height.max(1)).min(2);
                let (b, g, r) = match band {
                    0 => (0x20, 0x40, 0xd0),
                    1 => (0x40, 0xd0, 0x20),
                    _ => (0xd0, 0x20, 0x40),
                };
                pixels.extend_from_slice(&[b, g, r, 0xff]);
            }
        }
        pixels
    }
}
