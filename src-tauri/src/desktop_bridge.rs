use reqwest::header::{ACCEPT, USER_AGENT};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    fs::{create_dir_all, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{AppHandle, Manager, State, Url};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_notification::{NotificationExt, PermissionState};
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_updater::{Update, UpdaterExt};

#[cfg(windows)]
use std::process::Command;

#[cfg(windows)]
const APP_USER_MODEL_ID: &str = "com.lingkongran.pisper";
const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/ling-kong-ran/pisper/releases/latest";
const RELEASES_URL: &str = "https://github.com/ling-kong-ran/pisper/releases";
const UPDATE_MANIFEST_URL: &str =
    "https://github.com/ling-kong-ran/pisper/releases/latest/download/latest.json";
pub const UPDATER_PUBLIC_KEY: &str = match option_env!("PISPER_TAURI_UPDATER_PUBLIC_KEY") {
    Some(value) => value,
    None => "",
};

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub state: String,
    pub checked_at: Option<String>,
    pub message: String,
    pub release_url: String,
    pub can_download: bool,
    pub can_install: bool,
    pub can_resume: bool,
    pub notes: String,
    pub available_version: String,
    pub percent: f64,
    pub total: u64,
    pub transferred: u64,
    pub release_date: Option<String>,
}

impl UpdateStatus {
    fn idle() -> Self {
        Self {
            state: "idle".into(),
            release_url: RELEASES_URL.into(),
            ..Default::default()
        }
    }
}

#[derive(Clone)]
struct PendingUpdate {
    update: Update,
    bytes: Option<Vec<u8>>,
}

pub struct DesktopUpdateState {
    status: Arc<Mutex<UpdateStatus>>,
    pending: Arc<Mutex<Option<PendingUpdate>>>,
}

impl Default for DesktopUpdateState {
    fn default() -> Self {
        Self {
            status: Arc::new(Mutex::new(UpdateStatus::idle())),
            pending: Arc::new(Mutex::new(None)),
        }
    }
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: Option<String>,
    published_at: Option<String>,
    created_at: Option<String>,
    body: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    desktop: bool,
    packaged: bool,
    version: String,
    platform: &'static str,
    arch: &'static str,
    releases_url: &'static str,
    update: UpdateStatus,
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

fn version(value: &str) -> Option<Version> {
    Version::parse(value.trim().trim_start_matches(['v', 'V'])).ok()
}

fn checked_at() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn set_status(state: &DesktopUpdateState, value: UpdateStatus) -> UpdateStatus {
    *state.status.lock().expect("update status poisoned") = value.clone();
    value
}

fn update_log_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_log_dir()
        .ok()
        .map(|directory| directory.join("webview-updater.log"))
}

fn log_update(app: &AppHandle, message: &str) {
    let Some(path) = update_log_path(app) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = create_dir_all(parent);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{} {}", checked_at(), message);
    }
}

async fn latest_release(app: &AppHandle) -> Result<GitHubRelease, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| error.to_string())?;
    client
        .get(LATEST_RELEASE_API)
        .header(ACCEPT, "application/vnd.github+json")
        .header(
            USER_AGENT,
            format!("Pisper/{} (Tauri)", app.package_info().version),
        )
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())
}

async fn signed_update(app: &AppHandle) -> Result<Option<Update>, String> {
    if UPDATER_PUBLIC_KEY.trim().is_empty() || cfg!(debug_assertions) {
        return Ok(None);
    }
    let endpoint = Url::parse(UPDATE_MANIFEST_URL).map_err(|error| error.to_string())?;
    let cleanup_app = app.clone();
    app.updater_builder()
        .endpoints(vec![endpoint])
        .map_err(|error| error.to_string())?
        .on_before_exit(move || {
            crate::stop_sidecar(&cleanup_app);
            cleanup_app.cleanup_before_exit();
        })
        .build()
        .map_err(|error| error.to_string())?
        .check()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn desktop_get_app_info(app: AppHandle, state: State<'_, DesktopUpdateState>) -> AppInfo {
    AppInfo {
        desktop: true,
        packaged: !cfg!(debug_assertions),
        version: app.package_info().version.to_string(),
        platform: platform(),
        arch: arch(),
        releases_url: RELEASES_URL,
        update: state.status.lock().expect("update status poisoned").clone(),
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
pub fn desktop_set_language(app: AppHandle, language: String) -> String {
    let normalized = match language.as_str() {
        "en-US" => "en-US",
        _ => "zh-CN",
    };
    crate::set_tray_language(&app, normalized);
    normalized.into()
}

#[tauri::command]
pub fn desktop_update_status(state: State<'_, DesktopUpdateState>) -> UpdateStatus {
    state.status.lock().expect("update status poisoned").clone()
}

#[tauri::command]
pub async fn desktop_check_for_updates(
    app: AppHandle,
    state: State<'_, DesktopUpdateState>,
) -> Result<UpdateStatus, String> {
    set_status(
        &state,
        UpdateStatus {
            state: "checking".into(),
            release_url: RELEASES_URL.into(),
            ..Default::default()
        },
    );
    *state.pending.lock().expect("pending update poisoned") = None;

    let result = async {
        let release = latest_release(&app).await?;
        let current = app.package_info().version.clone();
        let latest = version(&release.tag_name)
            .ok_or_else(|| "GitHub release version is invalid.".to_string())?;
        let available = latest > current;
        let release_url = release.html_url.unwrap_or_else(|| RELEASES_URL.into());
        let release_date = release.published_at.or(release.created_at);
        let notes = release.body.unwrap_or_default();

        if !available {
            return Ok(UpdateStatus {
                state: "current".into(),
                checked_at: Some(checked_at()),
                message: "当前已是最新版本。".into(),
                release_url,
                notes,
                available_version: latest.to_string(),
                release_date,
                ..Default::default()
            });
        }

        let signed = signed_update(&app).await.unwrap_or(None);
        let metadata_matches = signed
            .as_ref()
            .is_some_and(|update| version(&update.version).as_ref() == Some(&latest));
        if metadata_matches {
            *state.pending.lock().expect("pending update poisoned") =
                signed.map(|update| PendingUpdate {
                    update,
                    bytes: None,
                });
        }

        Ok(UpdateStatus {
            state: "available".into(),
            checked_at: Some(checked_at()),
            message: if metadata_matches {
                String::new()
            } else if UPDATER_PUBLIC_KEY.trim().is_empty() {
                "此构建未配置更新签名公钥，请从 GitHub Releases 下载。".into()
            } else {
                "已发现更新，但签名安装元数据尚未同步。请稍后重试或从 GitHub Releases 下载。".into()
            },
            release_url,
            can_download: metadata_matches,
            notes,
            available_version: latest.to_string(),
            release_date,
            ..Default::default()
        })
    }
    .await;

    Ok(match result {
        Ok(status) => {
            log_update(&app, &format!("Update check completed: {}", status.state));
            set_status(&state, status)
        }
        Err(error) => {
            log_update(&app, &format!("Update check failed: {error}"));
            set_status(
                &state,
                UpdateStatus {
                    state: "error".into(),
                    checked_at: Some(checked_at()),
                    message: error,
                    release_url: RELEASES_URL.into(),
                    ..Default::default()
                },
            )
        }
    })
}

#[tauri::command]
pub async fn desktop_download_update(
    app: AppHandle,
    state: State<'_, DesktopUpdateState>,
) -> Result<UpdateStatus, String> {
    let pending = state
        .pending
        .lock()
        .expect("pending update poisoned")
        .clone();
    let Some(pending) = pending else {
        return Ok(state.status.lock().expect("update status poisoned").clone());
    };

    let mut downloading = state.status.lock().expect("update status poisoned").clone();
    downloading.state = "downloading".into();
    downloading.can_download = false;
    downloading.can_install = false;
    downloading.message.clear();
    downloading.percent = 0.0;
    downloading.transferred = 0;
    downloading.total = 0;
    set_status(&state, downloading);

    let status = state.status.clone();
    let mut transferred = 0_u64;
    let bytes = pending
        .update
        .download(
            move |chunk, total| {
                transferred = transferred.saturating_add(chunk as u64);
                let mut current = status.lock().expect("update status poisoned");
                current.transferred = transferred;
                current.total = total.unwrap_or_default();
                current.percent = if current.total > 0 {
                    transferred as f64 * 100.0 / current.total as f64
                } else {
                    0.0
                };
            },
            || {},
        )
        .await;

    Ok(match bytes {
        Ok(bytes) => {
            *state.pending.lock().expect("pending update poisoned") = Some(PendingUpdate {
                update: pending.update,
                bytes: Some(bytes),
            });
            let mut downloaded = state.status.lock().expect("update status poisoned").clone();
            downloaded.state = "downloaded".into();
            downloaded.can_install = true;
            downloaded.percent = 100.0;
            downloaded.message = "更新已下载，重启后完成安装。".into();
            log_update(&app, "Signed update downloaded and verified.");
            set_status(&state, downloaded)
        }
        Err(error) => {
            log_update(&app, &format!("Update download failed: {error}"));
            let mut failed = state.status.lock().expect("update status poisoned").clone();
            failed.state = "error".into();
            failed.message = error.to_string();
            failed.can_download = true;
            failed.can_resume = false;
            set_status(&state, failed)
        }
    })
}

#[tauri::command]
pub fn desktop_install_update(app: AppHandle, state: State<'_, DesktopUpdateState>) -> bool {
    let pending = state
        .pending
        .lock()
        .expect("pending update poisoned")
        .take();
    let Some(PendingUpdate {
        update,
        bytes: Some(bytes),
    }) = pending
    else {
        return false;
    };
    log_update(&app, "Installing verified update.");
    match update.install(bytes) {
        Ok(()) => app.restart(),
        Err(error) => {
            log_update(&app, &format!("Update installation failed: {error}"));
            false
        }
    }
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
pub fn desktop_open_releases(app: AppHandle, state: State<'_, DesktopUpdateState>) -> bool {
    let url = state
        .status
        .lock()
        .expect("update status poisoned")
        .release_url
        .clone();
    desktop_open_url(app, url)
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
