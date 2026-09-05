use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use std::{
    fs::{canonicalize, create_dir_all, write as write_file, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager, Url};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_notification::{NotificationExt, PermissionState};
use tauri_plugin_opener::OpenerExt;

#[cfg(windows)]
use std::process::Command;

#[cfg(windows)]
const APP_USER_MODEL_ID: &str = "com.lingkongran.pisper";
const RELEASES_URL: &str = "https://github.com/ling-kong-ran/pisper/releases";
pub const UPDATER_PUBLIC_KEY: &str = match option_env!("PISPER_TAURI_UPDATER_PUBLIC_KEY") {
    Some(value) => value,
    None => "",
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    desktop: bool,
    packaged: bool,
    version: String,
    platform: &'static str,
    arch: &'static str,
    releases_url: &'static str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetOpenInput {
    name: String,
    data: String,
}

const OPEN_ASSETS_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const OPEN_ASSETS_MAX_BYTES: u64 = 512 * 1024 * 1024;

fn cleanup_open_assets_at(root: &Path, now: SystemTime, ttl: Duration, max_bytes: u64) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut candidates = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false)
            || !entry
                .file_name()
                .to_string_lossy()
                .chars()
                .all(|value| value.is_ascii_digit())
        {
            continue;
        }
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        let modified = metadata.modified().unwrap_or(now);
        let age = now.duration_since(modified).unwrap_or_default();
        if age < ttl {
            continue;
        }
        let size = std::fs::read_dir(&path)
            .into_iter()
            .flatten()
            .filter_map(|file| file.ok())
            .filter_map(|file| file.metadata().ok())
            .filter(|file| file.is_file())
            .map(|file| file.len())
            .sum::<u64>();
        candidates.push((modified, size, path));
    }
    candidates.sort_by_key(|(modified, _, _)| *modified);
    let mut total = candidates.iter().map(|(_, size, _)| *size).sum::<u64>();
    for (_, size, path) in candidates {
        if total <= max_bytes {
            break;
        }
        if std::fs::remove_dir_all(path).is_ok() {
            total = total.saturating_sub(size);
        }
    }
}

pub(crate) fn cleanup_open_assets(app: &AppHandle) {
    let Ok(root) = app.path().app_cache_dir() else {
        return;
    };
    cleanup_open_assets_at(
        &root.join("open-assets"),
        SystemTime::now(),
        OPEN_ASSETS_TTL,
        OPEN_ASSETS_MAX_BYTES,
    );
}

#[derive(Deserialize)]
pub struct NotificationInput {
    title: String,
    body: String,
}

#[derive(Serialize)]
pub struct NotificationStatus {
    supported: bool,
    permission: &'static str,
    reason: &'static str,
}

#[derive(Serialize)]
pub struct NotificationResult {
    shown: bool,
    supported: bool,
    permission: &'static str,
    reason: &'static str,
}

fn platform() -> &'static str {
    match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        value => value,
    }
}

fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        value => value,
    }
}

fn clipped(value: &str, limit: usize) -> String {
    value.trim().chars().take(limit).collect()
}

fn checked_at() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn update_log_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_log_dir()
        .ok()
        .map(|directory| directory.join("component-updater.log"))
}

fn write_update_log(app: &AppHandle, channel: &str, message: &str) {
    let Some(path) = update_log_path(app) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = create_dir_all(parent);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{} [{channel}] {message}", checked_at());
    }
}

pub(crate) fn log_component_update(app: &AppHandle, message: &str) {
    write_update_log(app, "component-update", message);
}

#[tauri::command]
pub fn desktop_get_app_info(app: AppHandle) -> AppInfo {
    AppInfo {
        desktop: true,
        packaged: !cfg!(debug_assertions),
        version: app.package_info().version.to_string(),
        platform: platform(),
        arch: arch(),
        releases_url: RELEASES_URL,
    }
}

#[tauri::command]
pub async fn desktop_pick_directory(
    app: AppHandle,
    initial_directory: Option<String>,
) -> Result<Option<String>, String> {
    let mut dialog = app.dialog().file();
    if let Some(initial_directory) = initial_directory.filter(|value| !value.trim().is_empty()) {
        let path = PathBuf::from(initial_directory);
        if path.is_dir() {
            dialog = dialog.set_directory(path);
        }
    }
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    dialog
        .blocking_pick_folder()
        .map(|path| {
            path.simplified()
                .into_path()
                .map(|path| path.to_string_lossy().into_owned())
                .map_err(|error| error.to_string())
        })
        .transpose()
}

#[tauri::command]
pub async fn desktop_pick_files(app: AppHandle, initial_directory: Option<String>) -> Vec<String> {
    let mut dialog = app.dialog().file();
    if let Some(initial_directory) = initial_directory.filter(|value| !value.trim().is_empty()) {
        let path = PathBuf::from(initial_directory);
        if path.is_dir() {
            dialog = dialog.set_directory(path);
        }
    }
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    dialog
        .blocking_pick_files()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|path| path.simplified().into_path().ok())
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

#[tauri::command]
pub fn desktop_set_language(app: AppHandle, language: String) -> String {
    let normalized = match language.as_str() {
        "en-US" => "en-US",
        _ => "zh-CN",
    };
    crate::desktop_shell::set_tray_language(&app, normalized);
    normalized.into()
}

#[tauri::command]
pub fn desktop_open_url(app: AppHandle, url: String) -> bool {
    let Ok(parsed) = Url::parse(&url) else {
        return false;
    };
    if !matches!(parsed.scheme(), "http" | "https" | "mailto") {
        return false;
    }
    app.opener()
        .open_url(parsed.to_string(), None::<&str>)
        .is_ok()
}

#[tauri::command]
pub fn desktop_open_asset(app: AppHandle, input: AssetOpenInput) -> Result<bool, String> {
    let name = input.name.trim();
    if name.is_empty()
        || name.chars().count() > 180
        || name.chars().any(char::is_control)
        || name.contains(['/', '\\'])
    {
        return Err("资产文件名无效。".into());
    }
    if input.data.len() > 180 * 1024 * 1024 {
        return Err("资产超过 128 MB 原生打开限制。".into());
    }
    let bytes = BASE64_STANDARD
        .decode(input.data)
        .map_err(|_| "资产内容不是有效的 base64 数据。".to_string())?;
    if bytes.is_empty() || bytes.len() > 128 * 1024 * 1024 {
        return Err("资产内容为空或超过 128 MB 原生打开限制。".into());
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let directory = app
        .path()
        .app_cache_dir()
        .map_err(|error| error.to_string())?
        .join("open-assets")
        .join(nonce.to_string());
    create_dir_all(&directory).map_err(|error| error.to_string())?;
    let path = directory.join(name);
    write_file(&path, bytes).map_err(|error| error.to_string())?;
    app.opener()
        .open_path(path.to_string_lossy().into_owned(), None::<&str>)
        .map(|_| true)
        .map_err(|error| error.to_string())
}

fn canonical_local_path(value: &str) -> Result<PathBuf, String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 32_768 || value.chars().any(char::is_control) {
        return Err("本地路径无效。".into());
    }
    let requested = PathBuf::from(value);
    if !requested.is_absolute() {
        return Err("本地路径必须是绝对路径。".into());
    }
    let path = canonicalize(requested).map_err(|error| format!("本地路径不可访问：{error}"))?;
    if !path.is_file() && !path.is_dir() {
        return Err("本地路径不是文件或目录。".into());
    }
    Ok(path)
}

#[tauri::command]
pub fn desktop_reveal_path(app: AppHandle, path: String) -> Result<bool, String> {
    let path = canonical_local_path(&path)?;
    let result = if path.is_dir() {
        app.opener()
            .open_path(path.to_string_lossy().into_owned(), None::<&str>)
    } else {
        // 部分 Windows 文件关联实现不支持 reveal，退回打开父目录仍能保证用户到达目标文件。
        match app.opener().reveal_item_in_dir(&path) {
            Ok(()) => return Ok(true),
            Err(_) => app.opener().open_path(
                path.parent()
                    .unwrap_or_else(|| Path::new("."))
                    .to_string_lossy()
                    .into_owned(),
                None::<&str>,
            ),
        }
    };
    result.map(|_| true).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn desktop_open_releases(app: AppHandle) -> bool {
    desktop_open_url(app, RELEASES_URL.into())
}

#[tauri::command]
pub fn desktop_open_update_log(app: AppHandle) -> bool {
    let Some(path) = update_log_path(&app) else {
        return false;
    };
    if !path.exists() {
        return false;
    }
    app.opener().reveal_item_in_dir(path).is_ok()
}

#[cfg(windows)]
fn registry_dword(key: &str, name: &str) -> Option<u32> {
    let output = Command::new("reg.exe")
        .args(["query", key, "/v", name])
        .creation_flags(0x0800_0000)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text
        .lines()
        .find(|line| line.split_whitespace().next() == Some(name))?;
    let value = line.split_whitespace().last()?;
    if let Some(hex) = value.strip_prefix("0x") {
        u32::from_str_radix(hex, 16).ok()
    } else {
        value.parse().ok()
    }
}

#[cfg(windows)]
use std::os::windows::process::CommandExt;

fn notification_status(app: &AppHandle) -> NotificationStatus {
    let plugin_granted = matches!(
        app.notification().permission_state(),
        Ok(PermissionState::Granted) | Ok(PermissionState::Prompt)
    );
    if !plugin_granted {
        return NotificationStatus {
            supported: true,
            permission: "denied",
            reason: "app-disabled",
        };
    }

    #[cfg(windows)]
    {
        let global_key =
            "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Notifications\\Settings";
        let push_key = "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\PushNotifications";
        let app_key = format!("{global_key}\\{APP_USER_MODEL_ID}");
        if registry_dword(push_key, "ToastEnabled") == Some(0)
            || registry_dword(global_key, "NOC_GLOBAL_SETTING_TOASTS_ENABLED") == Some(0)
        {
            return NotificationStatus {
                supported: true,
                permission: "denied",
                reason: "system-disabled",
            };
        }
        if registry_dword(&app_key, "Enabled") == Some(0) {
            return NotificationStatus {
                supported: true,
                permission: "denied",
                reason: "app-disabled",
            };
        }
    }

    NotificationStatus {
        supported: true,
        permission: "granted",
        reason: "",
    }
}

#[tauri::command]
pub fn desktop_get_notification_status(app: AppHandle) -> NotificationStatus {
    notification_status(&app)
}

#[tauri::command]
pub fn desktop_open_notification_settings(app: AppHandle) -> bool {
    #[cfg(windows)]
    {
        app.opener()
            .open_url("ms-settings:notifications", None::<&str>)
            .is_ok()
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        false
    }
}

#[tauri::command]
pub fn desktop_show_notification(app: AppHandle, input: NotificationInput) -> NotificationResult {
    let status = notification_status(&app);
    if status.permission != "granted" {
        return NotificationResult {
            shown: false,
            supported: status.supported,
            permission: status.permission,
            reason: status.reason,
        };
    }
    let title = clipped(&input.title, 120);
    let body = clipped(&input.body, 2_000);
    if title.is_empty() {
        return NotificationResult {
            shown: false,
            supported: true,
            permission: "granted",
            reason: "invalid-title",
        };
    }
    let shown = app
        .notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .is_ok();
    NotificationResult {
        shown,
        supported: true,
        permission: "granted",
        reason: if shown { "" } else { "show-failed" },
    }
}

#[cfg(test)]
mod tests {
    use super::{canonical_local_path, cleanup_open_assets_at};
    use std::time::{Duration, SystemTime};

    #[test]
    fn local_path_bridge_accepts_only_existing_absolute_paths() {
        let current = std::env::current_dir().expect("读取当前目录");
        let canonical =
            canonical_local_path(current.to_string_lossy().as_ref()).expect("当前目录应可规范化");
        assert!(canonical.is_dir());
        assert!(canonical_local_path("relative/path.txt").is_err());
        assert!(canonical_local_path("bad\0path").is_err());
        assert!(canonical_local_path(
            current
                .join("pisper-missing-path")
                .to_string_lossy()
                .as_ref()
        )
        .is_err());
    }

    #[test]
    fn open_assets_cleanup_removes_expired_numeric_directories_only() {
        let root =
            std::env::temp_dir().join(format!("pisper-open-assets-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("123")).expect("创建测试目录");
        std::fs::write(root.join("123").join("asset.bin"), b"asset").expect("写入测试资产");
        std::fs::create_dir_all(root.join("keep-me")).expect("创建非资产目录");
        cleanup_open_assets_at(
            &root,
            SystemTime::now() + Duration::from_secs(2),
            Duration::from_secs(1),
            0,
        );
        assert!(!root.join("123").exists());
        assert!(root.join("keep-me").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn open_assets_cleanup_protects_recent_directories() {
        let root = std::env::temp_dir().join(format!(
            "pisper-open-assets-recent-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("456")).expect("创建测试目录");
        std::fs::write(root.join("456").join("asset.bin"), b"asset").expect("写入测试资产");
        cleanup_open_assets_at(&root, SystemTime::now(), Duration::from_secs(3600), 0);
        assert!(root.join("456").exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
