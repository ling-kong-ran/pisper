//! Pisper 壳入口：按平台分发到桌面壳（sidecar + 托盘）或移动壳（本地代理 + 配对）。
//!
//! - 桌面端（Windows/macOS/Linux）：`desktop_shell`，拉起 Node sidecar 供系统 WebView 使用。
//! - 移动端（Android/iOS）：`mobile`，内置 TLS 指纹锁定的本地代理，WebView 经代理
//!   访问已配对的桌面 Runtime，不运行本机 Agent。

#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod desktop_shell;
// mobile 模块在所有平台编译：代理逻辑与平台无关，这样可以在桌面主机上
// 直接 cargo check / cargo test 验证移动端代理，无需 Android 工具链。
mod mobile;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn run() {
    desktop_shell::run()
}

#[cfg(any(target_os = "android", target_os = "ios"))]
pub fn run() {
    mobile::run_mobile()
}
