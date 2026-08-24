use serde::Serialize;
use tauri::AppHandle;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileAppUpdateStatus {
    state: String,
    checked_at: Option<String>,
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
    update: MobileAppUpdateStatus,
}

#[derive(Default)]
pub struct MobileUpdateState {
    _private: (),
}

fn current_status(version: &str) -> MobileAppUpdateStatus {
    MobileAppUpdateStatus {
        state: "current".into(),
        checked_at: None,
        message: String::new(),
        release_url: String::new(),
        download_url: String::new(),
        can_download: false,
        can_install: false,
        notes: String::new(),
        available_version: version.into(),
        release_date: String::new(),
    }
}

#[tauri::command]
pub async fn mobile_app_info(app: AppHandle) -> Result<MobileAppInfo, String> {
    let version = app.package_info().version.to_string();
    Ok(MobileAppInfo {
        desktop: false,
        mobile: true,
        packaged: true,
        version: version.clone(),
        platform: if cfg!(target_os = "ios") {
            "ios".into()
        } else {
            "android".into()
        },
        arch: match std::env::consts::ARCH {
            "aarch64" => "arm64".into(),
            value => value.into(),
        },
        releases_url: String::new(),
        update: current_status(&version),
    })
}

#[tauri::command]
pub async fn mobile_check_app_update(app: AppHandle) -> Result<MobileAppUpdateStatus, String> {
    Ok(current_status(&app.package_info().version.to_string()))
}

#[tauri::command]
pub fn mobile_open_app_update() -> Result<bool, String> {
    Err("商店版本由 Google Play 或 App Store 管理更新。".into())
}

pub fn start_automatic_checks(_app: AppHandle) {}

pub fn check_after_resume(_app: AppHandle) {}
