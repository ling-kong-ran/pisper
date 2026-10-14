use std::path::Path;
use tauri::AppHandle;
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

#[derive(Clone, Copy)]
pub(super) enum StartupStage {
    Resources,
    Runtime,
    Desktop,
}

pub(super) fn guidance(stage: StartupStage) -> &'static str {
    match stage {
        StartupStage::Resources => "PISPER_RESOURCES_MISSING\n安装文件缺失或无法读取。请重新安装对应系统的官方安装包，并检查安全软件是否隔离了文件。无需另装 Node.js、Python 或 Bash。\nRequired application files are missing or unreadable. Reinstall the official package for this system and check security-software quarantine. No separate Node.js, Python or Bash installation is needed.",
        StartupStage::Runtime => "PISPER_RUNTIME_START_FAILED\nPisper Runtime 无法启动。可能是随包运行时文件损坏、原生模块无法加载、系统不兼容或权限受限；这不表示正在下载模型。请检查安装完整性和安全软件拦截，重装仍失败时请反馈系统版本及启动错误码。\nThe bundled Pisper Runtime could not start. Check package integrity, native-module compatibility and security-software restrictions. This is not a model download. If reinstalling does not help, report your OS version and this error code.",
        StartupStage::Desktop => "PISPER_DESKTOP_INIT_FAILED\n桌面界面初始化失败，无法确认具体缺失依赖。Windows 请检查 WebView2，Linux 请检查 WebKitGTK、图形会话和发行版依赖；其他系统请检查安装完整性及系统兼容性。请反馈系统版本及此错误码。\nDesktop initialization failed; the exact dependency could not be identified. Check WebView2 on Windows; WebKitGTK, the graphical session and distribution dependencies on Linux; package integrity and OS compatibility on other systems. Report your OS version and this error code.",
    }
}

#[cfg(target_os = "windows")]
pub(super) const WEBVIEW2_GUIDANCE: &str = "PISPER_WEBVIEW2_UNAVAILABLE\nMicrosoft WebView2 缺失或无法加载，Pisper 需要它显示界面。可联网时请重新运行普通安装包；离线环境请在其他电脑下载完整离线包（*-offline-setup.exe）后拷贝安装。\nMicrosoft WebView2 is missing or cannot be loaded. Rerun the standard installer online, or transfer the full offline installer (*-offline-setup.exe) from another PC.\nhttps://github.com/ling-kong-ran/pisper/releases/latest";

pub(super) fn missing_bundled_file<'a>(
    executable_dir: &Path,
    resources: &Path,
    binary_name: &'a str,
) -> Option<&'a str> {
    if !executable_dir.join(binary_name).is_file() {
        return Some(binary_name);
    }
    [
        "sidecar-runtime/runtime/sidecar.mjs",
        "desktop/dist/index.html",
    ]
    .into_iter()
    .find(|relative| !resources.join(relative).is_file())
}

pub(super) fn show(app: &AppHandle, message: &str) {
    // WebView 创建失败时仍能显示系统原生对话框；不在主线程阻塞等待回调。
    let handle = app.clone();
    app.dialog()
        .message(message)
        .title("Pisper — 启动失败 / Startup failed")
        .kind(MessageDialogKind::Warning)
        .show(move |_| handle.exit(1));
}

#[cfg(target_os = "windows")]
pub(super) fn show_before_runtime(message: &str) {
    let title: Vec<u16> = "Pisper — 启动失败 / Startup failed"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let message: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
    // 此路径发生在 Tauri/插件初始化前，直接使用系统对话框，不依赖 WebView。
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            windows_sys::Win32::UI::WindowsAndMessaging::MB_OK
                | windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONWARNING,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resource_diagnostics_identify_missing_payload_without_personal_paths() {
        let root =
            std::env::temp_dir().join(format!("pisper-startup-files-{}", std::process::id()));
        std::fs::create_dir_all(root.join("sidecar-runtime/runtime")).unwrap();
        std::fs::create_dir_all(root.join("desktop/dist")).unwrap();
        assert_eq!(
            missing_bundled_file(&root, &root, "sidecar"),
            Some("sidecar")
        );
        std::fs::write(root.join("sidecar"), "binary").unwrap();
        assert_eq!(
            missing_bundled_file(&root, &root, "sidecar"),
            Some("sidecar-runtime/runtime/sidecar.mjs")
        );
        std::fs::write(root.join("sidecar-runtime/runtime/sidecar.mjs"), "runtime").unwrap();
        assert_eq!(
            missing_bundled_file(&root, &root, "sidecar"),
            Some("desktop/dist/index.html")
        );
        std::fs::write(root.join("desktop/dist/index.html"), "frontend").unwrap();
        assert_eq!(missing_bundled_file(&root, &root, "sidecar"), None);
        std::fs::remove_dir_all(root).unwrap();
    }
}
