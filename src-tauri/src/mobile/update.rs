use std::time::{Duration, Instant};

use semver::Version;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;

const APP_MANIFEST_URL: &str = "https://ling-kong-ran.github.io/pisper/latest-app.json";
const APP_RELEASES_URL: &str = "https://github.com/ling-kong-ran/pisper/releases?q=app-v";
const APP_RELEASE_PATH: &str = "/ling-kong-ran/pisper/releases";
const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const AUTOMATIC_CHECK_DELAY: Duration = Duration::from_secs(3);
const AUTOMATIC_CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const RECENT_CHECK_WINDOW: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileAppUpdateStatus {
    state: String,
    checked_at: String,
    message: String,
    release_url: String,
    download_url: String,
    can_download: bool,
    can_install: bool,
    notes: String,
    available_version: String,
    release_date: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileAppInfo {
    desktop: bool,
    mobile: bool,
    packaged: bool,
    version: String,
    platform: String,
    arch: String,
    releases_url: String,
    update: Option<MobileAppUpdateStatus>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppManifest {
    version: String,
    tag: String,
    url: String,
    #[serde(default)]
    apk: String,
    #[serde(default)]
    ipa: String,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    release_date: String,
}

#[derive(Default)]
struct MobileUpdateCache {
    status: Option<MobileAppUpdateStatus>,
    last_attempt: Option<Instant>,
    checking: bool,
    notified_version: Option<String>,
}

#[derive(Default)]
pub struct MobileUpdateState {
    cache: tokio::sync::Mutex<MobileUpdateCache>,
    completed: tokio::sync::Notify,
}

fn checked_at() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn platform() -> &'static str {
    #[cfg(target_os = "android")]
    {
        "android"
    }
    #[cfg(target_os = "ios")]
    {
        "ios"
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        std::env::consts::OS
    }
}

fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        value => value,
    }
}

fn platform_asset(manifest: &AppManifest) -> &str {
    #[cfg(target_os = "ios")]
    {
        &manifest.ipa
    }
    #[cfg(not(target_os = "ios"))]
    {
        &manifest.apk
    }
}

fn safe_asset_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && !value.contains('/')
        && !value.contains('\\')
        && !value.contains("..")
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".-_".contains(character))
}

fn validate_manifest(
    manifest: AppManifest,
    current_version: &str,
) -> Result<MobileAppUpdateStatus, String> {
    let current =
        Version::parse(current_version).map_err(|error| format!("当前 App 版本无效：{error}"))?;
    let latest = Version::parse(manifest.version.trim())
        .map_err(|error| format!("App 更新版本无效：{error}"))?;
    let expected_tag = format!("app-v{latest}");
    if manifest.tag != expected_tag {
        return Err("App 更新清单的版本与标签不一致。".into());
    }

    let expected_release_url =
        format!("https://github.com/ling-kong-ran/pisper/releases/tag/{expected_tag}");
    if manifest.url != expected_release_url {
        return Err("App 更新清单包含不受信任的发布地址。".into());
    }

    for asset in [&manifest.apk, &manifest.ipa] {
        if !asset.is_empty() && !safe_asset_name(asset) {
            return Err("App 更新清单包含无效的安装包名称。".into());
        }
    }
    let asset = platform_asset(&manifest);
    let available = latest > current;
    if available && asset.is_empty() {
        return Err("App 更新清单缺少当前平台的安装包。".into());
    }
    let download_url = if asset.is_empty() {
        String::new()
    } else {
        format!("https://github.com/ling-kong-ran/pisper/releases/download/{expected_tag}/{asset}")
    };
    Ok(MobileAppUpdateStatus {
        state: if available { "available" } else { "current" }.into(),
        checked_at: checked_at(),
        message: String::new(),
        release_url: expected_release_url,
        download_url,
        can_download: available,
        // Android 和 unsigned iOS 都必须交给系统或重签工具完成安装。
        can_install: false,
        notes: manifest.notes,
        available_version: latest.to_string(),
        release_date: manifest.release_date,
    })
}

async fn fetch_update(current_version: &str) -> Result<MobileAppUpdateStatus, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| format!("无法初始化 App 更新检查：{error}"))?;
    let response = client
        .get(APP_MANIFEST_URL)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|error| format!("无法获取 App 更新清单：{error}"))?;
    if !response.status().is_success() {
        return Err(format!("App 更新检查失败：HTTP {}", response.status()));
    }
    if response.content_length().unwrap_or(0) > MAX_MANIFEST_BYTES as u64 {
        return Err("App 更新清单过大。".into());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("无法读取 App 更新清单：{error}"))?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err("App 更新清单过大。".into());
    }
    let manifest = serde_json::from_slice::<AppManifest>(&bytes)
        .map_err(|error| format!("App 更新清单格式无效：{error}"))?;
    validate_manifest(manifest, current_version)
}

async fn finish_check(state: &MobileUpdateState, result: &Result<MobileAppUpdateStatus, String>) {
    let mut cache = state.cache.lock().await;
    cache.last_attempt = Some(Instant::now());
    cache.checking = false;
    if let Ok(status) = result {
        cache.status = Some(status.clone());
    }
    drop(cache);
    state.completed.notify_waiters();
}

async fn check_with_cache(
    app: &AppHandle,
    state: &MobileUpdateState,
    refresh: bool,
) -> Result<MobileAppUpdateStatus, String> {
    loop {
        let completed = state.completed.notified();
        {
            let mut cache = state.cache.lock().await;
            if !refresh
                && cache
                    .last_attempt
                    .is_some_and(|started| started.elapsed() < RECENT_CHECK_WINDOW)
            {
                if let Some(status) = cache.status.clone() {
                    return Ok(status);
                }
            }
            if !cache.checking {
                cache.checking = true;
                break;
            }
        }
        completed.await;
    }

    let result = fetch_update(&app.package_info().version.to_string()).await;
    finish_check(state, &result).await;
    result
}

#[tauri::command]
pub async fn mobile_app_info(
    app: AppHandle,
    state: tauri::State<'_, MobileUpdateState>,
) -> Result<MobileAppInfo, String> {
    Ok(MobileAppInfo {
        desktop: false,
        mobile: true,
        packaged: true,
        version: app.package_info().version.to_string(),
        platform: platform().into(),
        arch: arch().into(),
        releases_url: APP_RELEASES_URL.into(),
        update: state.cache.lock().await.status.clone(),
    })
}

#[tauri::command]
pub async fn mobile_check_app_update(
    refresh: Option<bool>,
    app: AppHandle,
    state: tauri::State<'_, MobileUpdateState>,
) -> Result<MobileAppUpdateStatus, String> {
    check_with_cache(&app, &state, refresh.unwrap_or(true)).await
}

fn is_allowed_update_url(value: &str) -> bool {
    tauri::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str() == Some("github.com")
            && (url.path() == APP_RELEASE_PATH
                || url
                    .path()
                    .strip_prefix(APP_RELEASE_PATH)
                    .is_some_and(|suffix| suffix.starts_with('/')))
    })
}

#[tauri::command]
pub fn mobile_open_app_update(app: AppHandle, url: String) -> Result<bool, String> {
    if !is_allowed_update_url(&url) {
        return Err("拒绝打开不受信任的 App 更新地址。".into());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map(|_| true)
        .map_err(|error| format!("无法打开 App 更新地址：{error}"))
}

#[cfg(any(target_os = "android", target_os = "ios"))]
fn notify_available(app: &AppHandle, status: &MobileAppUpdateStatus) {
    use tauri::plugin::PermissionState;
    use tauri_plugin_notification::NotificationExt;

    if !matches!(
        app.notification().permission_state(),
        Ok(PermissionState::Granted)
    ) {
        return;
    }
    let _ = app
        .notification()
        .builder()
        .id(5102)
        .title("Pisper App update available")
        .body(format!(
            "Pisper App v{} 可更新 / Update available",
            status.available_version
        ))
        .auto_cancel()
        .show();
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn notify_available(_app: &AppHandle, _status: &MobileAppUpdateStatus) {}

async fn automatic_check(app: AppHandle) {
    let state = app.state::<MobileUpdateState>();
    {
        let mut cache = state.cache.lock().await;
        if cache.checking
            || cache
                .last_attempt
                .is_some_and(|started| started.elapsed() < AUTOMATIC_CHECK_INTERVAL)
        {
            return;
        }
        cache.checking = true;
    }

    let result = fetch_update(&app.package_info().version.to_string()).await;
    finish_check(&state, &result).await;
    let Ok(status) = result else {
        return;
    };
    if status.state != "available" {
        return;
    }

    let should_notify = {
        let mut cache = state.cache.lock().await;
        if cache.notified_version.as_deref() == Some(&status.available_version) {
            false
        } else {
            cache.notified_version = Some(status.available_version.clone());
            true
        }
    };
    if should_notify {
        notify_available(&app, &status);
    }
}

pub fn start_automatic_checks(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(AUTOMATIC_CHECK_DELAY).await;
        loop {
            automatic_check(app.clone()).await;
            tokio::time::sleep(AUTOMATIC_CHECK_INTERVAL).await;
        }
    });
}

pub fn check_after_resume(app: AppHandle) {
    tauri::async_runtime::spawn(automatic_check(app));
}

#[cfg(test)]
mod tests {
    use super::{is_allowed_update_url, validate_manifest, AppManifest};

    fn manifest(version: &str) -> AppManifest {
        AppManifest {
            version: version.into(),
            tag: format!("app-v{version}"),
            url: format!("https://github.com/ling-kong-ran/pisper/releases/tag/app-v{version}"),
            apk: "app-universal-release-signed.apk".into(),
            ipa: "pisper-ios-unsigned.ipa".into(),
            notes: "- 新增 App 更新检查".into(),
            release_date: "2026-08-21".into(),
        }
    }

    #[test]
    fn resolves_available_app_release() {
        let status = validate_manifest(manifest("0.2.0"), "0.1.1").expect("valid manifest");
        assert_eq!(status.state, "available");
        assert_eq!(status.available_version, "0.2.0");
        assert!(status.can_download);
        assert!(!status.can_install);
        assert!(status.download_url.contains("/app-v0.2.0/"));
    }

    #[test]
    fn accepts_legacy_current_manifest_without_ipa() {
        let value = serde_json::json!({
            "version": "0.1.1",
            "tag": "app-v0.1.1",
            "url": "https://github.com/ling-kong-ran/pisper/releases/tag/app-v0.1.1",
            "apk": "app-universal-release-signed.apk"
        });
        let parsed = serde_json::from_value(value).expect("legacy manifest");
        let status = validate_manifest(parsed, "0.1.1").expect("current legacy manifest");
        assert_eq!(status.state, "current");
    }

    #[test]
    fn rejects_available_release_without_current_platform_asset() {
        let mut value = manifest("0.2.0");
        value.apk.clear();
        assert!(validate_manifest(value, "0.1.1").is_err());
    }

    #[test]
    fn rejects_untrusted_release_metadata() {
        let mut value = manifest("0.2.0");
        value.url = "https://example.com/app-v0.2.0".into();
        assert!(validate_manifest(value, "0.1.1").is_err());
        assert!(!is_allowed_update_url("https://example.com/update.apk"));
        assert!(!is_allowed_update_url(
            "https://github.com/ling-kong-ran/pisper/releases-malicious/app.apk"
        ));
        assert!(is_allowed_update_url(
            "https://github.com/ling-kong-ran/pisper/releases?q=app-v"
        ));
        assert!(is_allowed_update_url(
            "https://github.com/ling-kong-ran/pisper/releases/download/app-v0.2.0/app.apk"
        ));
    }
}
