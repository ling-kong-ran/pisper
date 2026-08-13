use serde::{Deserialize, Serialize};
use std::{
    fs::{create_dir_all, OpenOptions},
    io::Write,
    path::PathBuf,
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
    crate::set_tray_language(&app, normalized);
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
