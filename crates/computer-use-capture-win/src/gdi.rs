//! GDI PrintWindow 单帧捕获：WGC 不可用（旧系统/无 D3D）时的轮询兜底，
//! 对位 macOS 侧 CGWindowListCreateImage 轮询路径。

use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, HGDIOBJ,
};
// PrintWindow 在 win32metadata 中被归到 Storage::Xps（历史分类），非 WindowsAndMessaging。
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsWindow};

use crate::{CaptureError, CapturedFrame};

/// 与 macOS 轮询路径相同的尺寸保护：0 尺寸或超大窗口直接拒绝，避免整块分配失控。
const MAX_SIDE: i32 = 8192;

/// PW_RENDERFULLCONTENT：走 DWM 合成器渲染窗口内容。缺此标志时 DirectX/
/// 大多数现代窗口只能得到黑帧——这是 PrintWindow 兜底路径可用的关键。
const PW_RENDERFULLCONTENT: PRINT_WINDOW_FLAGS = PRINT_WINDOW_FLAGS(2);

/// 捕获指定 HWND 的整窗画面（含边框，与 WGC 输出范围一致），BGRA top-down。
///
/// # Safety 边界
/// 全部 Win32 调用在单一 unsafe 块内成对获取/释放 DC 与 GDI 对象；
/// 中途失败通过闭包统一走资源回收，不泄漏 GDI 句柄（GDI 句柄是进程级
/// 稀缺资源，泄漏会拖垮整个桌面会话）。
pub fn capture_window_gdi(window_id: u64) -> Result<CapturedFrame, CaptureError> {
    unsafe {
        let hwnd = HWND(window_id as isize as *mut c_void);
        if !IsWindow(Some(hwnd)).as_bool() {
            return Err(CaptureError::WindowMissing);
        }
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return Err(CaptureError::Other("GetWindowRect failed".into()));
        }
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        if width <= 0 || height <= 0 || width > MAX_SIDE || height > MAX_SIDE {
            return Err(CaptureError::Other(format!(
                "invalid window size {width}x{height}"
            )));
        }

        let screen_dc = GetDC(None);
        if screen_dc.is_invalid() {
            return Err(CaptureError::Backend("GetDC failed".into()));
        }
        // 内层闭包负责捕获与拷贝；无论成败，外层统一释放 screen_dc。
        let outcome = (|| -> Result<CapturedFrame, CaptureError> {
            let mem_dc = CreateCompatibleDC(Some(screen_dc));
            if mem_dc.is_invalid() {
                return Err(CaptureError::Backend("CreateCompatibleDC failed".into()));
            }
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    // 负高度 = top-down DIB：行序与 image crate 的坐标系一致，
                    // 编码管线无需翻转。
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: 0, // BI_RGB：32bpp 内存布局即 B,G,R,X
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let dib = CreateDIBSection(Some(mem_dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
                .map_err(|error| CaptureError::Backend(format!("CreateDIBSection: {error}")))?;
            let old = SelectObject(mem_dc, HGDIOBJ(dib.0));
            // PrintWindow 对最小化窗口返回 false；对隐藏 DirectX 内容可能成功但
            // 画面过时——与 macOS 轮询的已知局限一致，镜像面板文档已注明。
            let ok = PrintWindow(hwnd, mem_dc, PW_RENDERFULLCONTENT).as_bool();
            SelectObject(mem_dc, old);
            let frame = if ok && !bits.is_null() {
                let len = (width as usize) * (height as usize) * 4;
                // SAFETY: DIB section 保证 bits 指向 width*height*4 字节可读内存。
                let bgra = std::slice::from_raw_parts(bits as *const u8, len).to_vec();
                Some(CapturedFrame {
                    width: width as u32,
                    height: height as u32,
                    bgra,
                })
            } else {
                None
            };
            let _ = DeleteObject(HGDIOBJ(dib.0));
            let _ = DeleteDC(mem_dc);
            frame.ok_or_else(|| CaptureError::Other("PrintWindow failed".into()))
        })();
        ReleaseDC(None, screen_dc);
        outcome
    }
}
