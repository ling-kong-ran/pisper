//! Pisper Computer Use Windows 捕获后端（M3：Windows 原生镜像流对等）。
//!
//! 与 macOS 侧 SCStream 主路径 / CGWindowList 轮询兜底的双路径结构对齐：
//! - [`wgc`]：Windows.Graphics.Capture 连续镜像流（WinRT 运行时激活，零 WGC 强链接，
//!   旧系统激活失败由调用方优雅降级）
//! - [`gdi`]：PrintWindow 单帧轮询（无系统版本门槛的兜底）
//!
//! 本 crate 独立于主 crate（src-tauri）的原因：主 crate 在 macOS 主机上无法对
//! Windows 目标做 cargo check（ring 需要 C 交叉工具链）；拆出后本 crate 依赖仅
//! windows + std，可在任意主机 `cargo check --target x86_64-pc-windows-msvc` 验证。

/// 捕获的单帧 BGRA 位图。
///
/// 与主 crate `CapturedWindow` 布局一致：`bgra` 按行主序、每像素 4 字节
/// B,G,R,X，由平台无关的编码管线（签名跳过 + JPEG 编码）消费。
#[derive(Clone)]
pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// 捕获错误分类（与主 crate `CaptureError` 语义一致）。
#[derive(Debug, Clone)]
pub enum CaptureError {
    /// 目标窗口已销毁/最小化到不可恢复：调用方应停流并上报 `window_missing`。
    WindowMissing,
    /// 捕获后端自身故障（设备丢失、API 失败等）：调用方停流上报。
    Backend(String),
    /// 单帧瞬时失败：调用方可跳过该帧继续。
    Other(String),
}

impl CaptureError {
    /// 供事件序列化的文案。
    pub fn message(&self) -> String {
        match self {
            CaptureError::WindowMissing => "window missing".into(),
            CaptureError::Backend(reason) => format!("capture backend error: {reason}"),
            CaptureError::Other(reason) => reason.clone(),
        }
    }
}

#[cfg(windows)]
mod gdi;
#[cfg(windows)]
mod wgc;

#[cfg(windows)]
pub use gdi::capture_window_gdi;
#[cfg(windows)]
pub use wgc::{start_wgc, wgc_supported, FrameCallback, WgcStream};
