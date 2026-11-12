//! 桌面壳安装包的后台下载与“安装 / 在文件夹中显示”入口。
//!
//! 组件更新器换不了壳本身（这个 Rust 程序）。壳在启动后稍等片刻、之后每 6 小时检查一次最新发布，
//! 默认在后台下载并验签当前平台的安装包，放进 `<应用本地数据目录>/installers/`；安装由用户点“安装”，
//! 用系统打开安装包后自己完成，Pisper 不退出也不静默运行任何东西。下载、验签和清理的规则都在
//! `pisper_component_updater::shell_installer` 里，这里只负责调度、状态和桥接命令。

use pisper_component_updater::{
    rate_limited_until,
    shell_installer::{
        is_newer, release_url, DownloadedInstaller, InstallerPlatform, ShellInstallerSettings,
        ShellInstallerUpdater, ShellRelease, INSTALLERS_DIR,
    },
};
use semver::Version;
use serde::Serialize;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tauri::{AppHandle, Manager, WebviewWindow};
use tauri_plugin_opener::OpenerExt;

use crate::desktop_shell::desktop_bridge::{log_shell_installer, UPDATER_PUBLIC_KEY};

// 不和启动抢网络与磁盘：主窗口、Runtime 和组件更新检查（前端 15 秒后）都先跑起来。
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(45);
// 与前端组件更新检查的周期相同；releases/download 直链不占 API 额度。
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const MAX_MESSAGE_CHARS: usize = 500;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellInstallerStatus {
    supported: bool,
    /// unsupported | idle | checking | current | available | downloading | downloaded | error
    state: String,
    current_version: String,
    available_version: String,
    release_url: String,
    notes: String,
    file_name: String,
    size: u64,
    transferred: u64,
    /// 有可下载的已签名安装包（macOS 旧发布没有 dmg 签名时为 false）。
    can_download: bool,
    auto_download: bool,
    message: String,
    retry_at: Option<u64>,
}

/// 同一时刻只跑一轮检查或下载；守卫随任务结束释放。
struct BusyGuard(Arc<AtomicBool>);

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

#[derive(Default)]
pub struct ShellInstallerState {
    status: Mutex<Option<ShellInstallerStatus>>,
    downloaded: Mutex<Option<DownloadedInstaller>>,
    busy: Arc<AtomicBool>,
}

impl ShellInstallerState {
    fn try_begin(&self) -> Option<BusyGuard> {
        (!self.busy.swap(true, Ordering::SeqCst)).then(|| BusyGuard(self.busy.clone()))
    }

    // 基础状态只在第一次写入时才构造：下载进度每个数据块都会写，不能每次都去读设置文件。
    fn update(
        &self,
        change: impl FnOnce(&mut ShellInstallerStatus),
        base: impl FnOnce() -> ShellInstallerStatus,
    ) {
        let mut status = self.status.lock().expect("shell installer status poisoned");
        change(status.get_or_insert_with(base));
    }

    fn downloaded(&self) -> Option<DownloadedInstaller> {
        self.downloaded
            .lock()
            .expect("shell installer status poisoned")
            .clone()
    }

    fn set_downloaded(&self, installer: Option<DownloadedInstaller>) {
        *self
            .downloaded
            .lock()
            .expect("shell installer status poisoned") = installer;
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Trigger {
    Scheduled,
    Manual,
    Download,
}

fn installers_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_local_data_dir()
        .map(|directory| directory.join(INSTALLERS_DIR))
        .map_err(|error| error.to_string())
}

fn shell_version(app: &AppHandle) -> Version {
    Version::parse(&app.package_info().version.to_string())
        .unwrap_or_else(|_| Version::new(0, 0, 0))
}

fn updater(app: &AppHandle) -> Result<ShellInstallerUpdater, String> {
    let platform =
        InstallerPlatform::current().ok_or_else(|| "unsupported platform".to_string())?;
    ShellInstallerUpdater::new(
        installers_dir(app)?,
        platform,
        UPDATER_PUBLIC_KEY,
        &format!("Pisper-Desktop/{}", app.package_info().version),
    )
    .map_err(|error| format!("{error:#}"))
}

fn settings(app: &AppHandle) -> ShellInstallerSettings {
    installers_dir(app)
        .map(|directory| pisper_component_updater::shell_installer::read_settings(&directory))
        .unwrap_or_default()
}

fn base_status(app: &AppHandle) -> ShellInstallerStatus {
    let supported = InstallerPlatform::current().is_some();
    ShellInstallerStatus {
        supported,
        state: if supported { "idle" } else { "unsupported" }.into(),
        current_version: shell_version(app).to_string(),
        available_version: String::new(),
        release_url: "https://github.com/ling-kong-ran/pisper/releases".into(),
        notes: String::new(),
        file_name: String::new(),
        size: 0,
        transferred: 0,
        can_download: false,
        auto_download: settings(app).auto_download,
        message: String::new(),
        retry_at: None,
    }
}

fn snapshot(app: &AppHandle) -> ShellInstallerStatus {
    let state = app.state::<ShellInstallerState>();
    let status = state
        .status
        .lock()
        .expect("shell installer status poisoned")
        .clone();
    let base = base_status(app);
    match status {
        Some(status) => ShellInstallerStatus {
            auto_download: base.auto_download,
            current_version: base.current_version,
            ..status
        },
        None => base,
    }
}

fn set_status(app: &AppHandle, change: impl FnOnce(&mut ShellInstallerStatus)) {
    app.state::<ShellInstallerState>()
        .update(change, || base_status(app));
}

// 错误只给界面一段截短的英文原因；底层错误不带 URL 和个人路径（见 crate 里的错误上下文）。
fn clipped(message: &str) -> String {
    message.chars().take(MAX_MESSAGE_CHARS).collect()
}

fn downloaded_status(
    status: &mut ShellInstallerStatus,
    installer: &DownloadedInstaller,
    release: Option<&ShellRelease>,
) {
    status.state = "downloaded".into();
    status.available_version = installer.version.to_string();
    status.release_url = release
        .map(|release| release.release_url.clone())
        .unwrap_or_else(|| release_url(&installer.version));
    if let Some(release) = release {
        status.notes = release.notes.clone();
    }
    status.file_name = installer.file_name.clone();
    status.size = installer.size;
    status.transferred = installer.size;
    status.can_download = true;
    status.message.clear();
    status.retry_at = None;
}

async fn blocking<T: Send + 'static>(task: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    tauri::async_runtime::spawn_blocking(task).await.ok()
}

/// 启动时在后台整理目录：删掉已经装上的版本和没下完的临时文件，找回上次下好的安装包。
async fn restore(app: &AppHandle) {
    let state = app.state::<ShellInstallerState>();
    let Some(_guard) = state.try_begin() else {
        return;
    };
    let Ok(updater) = updater(app) else {
        return;
    };
    let current = shell_version(app);
    let Some(kept) = blocking(move || updater.prune(&current)).await.flatten() else {
        return;
    };
    log_shell_installer(
        app,
        &format!("found downloaded installer {}", kept.file_name),
    );
    state.set_downloaded(Some(kept.clone()));
    set_status(app, |status| downloaded_status(status, &kept, None));
}

async fn run_cycle(app: AppHandle, _guard: BusyGuard, trigger: Trigger) {
    let state = app.state::<ShellInstallerState>();
    let updater = match updater(&app) {
        Ok(updater) => updater,
        Err(error) => {
            set_status(&app, |status| {
                status.state = if status.supported {
                    "error"
                } else {
                    "unsupported"
                }
                .into();
                status.message = clipped(&error);
            });
            return;
        }
    };
    let current = shell_version(&app);
    let previous = state
        .downloaded()
        .filter(|installer| is_newer(&current, &installer.version));
    if previous.is_none() {
        set_status(&app, |status| {
            status.state = "checking".into();
            status.message.clear();
            status.retry_at = None;
        });
    }

    let release = match updater.fetch_release().await {
        Ok(release) => release,
        Err(error) => {
            log_shell_installer(&app, &format!("release check failed: {error:#}"));
            // 已经下好的安装包不因为一次检查失败而藏起来。
            if previous.is_some() {
                return;
            }
            let retry_at = rate_limited_until(&error);
            set_status(&app, |status| {
                status.state = "error".into();
                status.message = clipped(&format!("{error:#}"));
                status.retry_at = retry_at;
            });
            return;
        }
    };

    if !release.is_newer_than(&current) {
        // 用户已经装上了（或者本来就是最新）：留着的安装包没用了。
        let pruner = updater.clone();
        let pruned = current.clone();
        let _ = blocking(move || pruner.prune(&pruned)).await;
        state.set_downloaded(None);
        set_status(&app, |status| {
            status.state = "current".into();
            status.available_version = release.version.to_string();
            status.release_url = release.release_url.clone();
            status.notes.clear();
            status.file_name.clear();
            status.size = 0;
            status.transferred = 0;
            status.can_download = false;
            status.message.clear();
            status.retry_at = None;
        });
        return;
    }

    let available = |status: &mut ShellInstallerStatus, can_download: bool| {
        status.state = "available".into();
        status.available_version = release.version.to_string();
        status.release_url = release.release_url.clone();
        status.notes = release.notes.clone();
        status.file_name = release
            .installer
            .as_ref()
            .map(|asset| asset.file_name.clone())
            .unwrap_or_default();
        status.size = 0;
        status.transferred = 0;
        status.can_download = can_download;
        status.message.clear();
        status.retry_at = None;
    };
    if release.installer.is_none() {
        // 例如 macOS 的旧发布没有 dmg 签名：只提示有新版本，照旧去发布页下载。
        // 手里还有一个比当前壳新、已验签的安装包时，先让用户装它。
        if let Some(previous) = previous {
            set_status(&app, |status| downloaded_status(status, &previous, None));
            return;
        }
        state.set_downloaded(None);
        set_status(&app, |status| available(status, false));
        return;
    }

    let checker = updater.clone();
    let checked = current.clone();
    if let Some(existing) = blocking(move || checker.verified_installer(&checked))
        .await
        .flatten()
        .filter(|installer| installer.version == release.version)
    {
        state.set_downloaded(Some(existing.clone()));
        set_status(&app, |status| {
            downloaded_status(status, &existing, Some(&release))
        });
        return;
    }

    if !(trigger == Trigger::Download || updater.settings().auto_download) {
        set_status(&app, |status| available(status, true));
        return;
    }

    log_shell_installer(
        &app,
        &format!(
            "downloading installer for v{} ({})",
            release.version,
            if trigger == Trigger::Scheduled {
                "scheduled"
            } else {
                "requested"
            }
        ),
    );
    set_status(&app, |status| {
        available(status, true);
        status.state = "downloading".into();
    });
    let progress_app = app.clone();
    let result = updater
        .download(&release, move |transferred, total| {
            set_status(&progress_app, |status| {
                status.transferred = transferred;
                status.size = total;
            });
        })
        .await;
    match result {
        Ok(installer) => {
            log_shell_installer(
                &app,
                &format!(
                    "downloaded and verified {} ({} bytes)",
                    installer.file_name, installer.size
                ),
            );
            state.set_downloaded(Some(installer.clone()));
            set_status(&app, |status| {
                downloaded_status(status, &installer, Some(&release))
            });
        }
        Err(error) => {
            log_shell_installer(&app, &format!("installer download failed: {error:#}"));
            // 手里还有一个比当前壳新的安装包时继续给用户装它，下一轮再试新版本。
            if let Some(previous) = previous {
                set_status(&app, |status| downloaded_status(status, &previous, None));
                return;
            }
            let retry_at = rate_limited_until(&error);
            set_status(&app, |status| {
                available(status, true);
                status.state = "error".into();
                status.message = clipped(&format!("{error:#}"));
                status.retry_at = retry_at;
            });
        }
    }
}

fn spawn_cycle(app: &AppHandle, guard: BusyGuard, trigger: Trigger) {
    let app = app.clone();
    tauri::async_runtime::spawn(run_cycle(app, guard, trigger));
}

/// 启动后调用一次：立即在后台整理目录，稍后开始定时检查。开发构建只整理、不自动下载。
pub fn start(app: &AppHandle) {
    if InstallerPlatform::current().is_none() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        restore(&app).await;
        if cfg!(debug_assertions) {
            return;
        }
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        loop {
            if let Some(guard) = app.state::<ShellInstallerState>().try_begin() {
                run_cycle(app.clone(), guard, Trigger::Scheduled).await;
            }
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

// 与其他桥接命令一样只给主窗口；桌面宠物窗口和远程工作区窗口都拿不到（另见 capabilities）。
fn require_main(window: &WebviewWindow) -> Result<(), String> {
    if window.label() != "main" {
        return Err("Desktop installer controls require the main window".into());
    }
    Ok(())
}

fn begin(app: &AppHandle, trigger: Trigger) {
    let state = app.state::<ShellInstallerState>();
    let Some(guard) = state.try_begin() else {
        // 已经有一轮在跑，它的结果就是这次请求要的结果。
        return;
    };
    if state
        .downloaded()
        .is_none_or(|installer| !is_newer(&shell_version(app), &installer.version))
    {
        set_status(app, |status| {
            status.state = if trigger == Trigger::Download {
                "downloading"
            } else {
                "checking"
            }
            .into();
            status.message.clear();
            status.retry_at = None;
        });
    }
    spawn_cycle(app, guard, trigger);
}

#[tauri::command]
pub fn desktop_shell_installer_status(
    window: WebviewWindow,
    app: AppHandle,
) -> Result<ShellInstallerStatus, String> {
    require_main(&window)?;
    Ok(snapshot(&app))
}

/// 用户点“检查更新”：在后台检查，开着自动下载时顺带下载；界面轮询状态看结果。
#[tauri::command]
pub fn desktop_check_shell_installer(
    window: WebviewWindow,
    app: AppHandle,
) -> Result<ShellInstallerStatus, String> {
    require_main(&window)?;
    if InstallerPlatform::current().is_some() {
        begin(&app, Trigger::Manual);
    }
    Ok(snapshot(&app))
}

/// 关了自动下载时，用户手动要这一个安装包。
#[tauri::command]
pub fn desktop_download_shell_installer(
    window: WebviewWindow,
    app: AppHandle,
) -> Result<ShellInstallerStatus, String> {
    require_main(&window)?;
    if InstallerPlatform::current().is_some() {
        begin(&app, Trigger::Download);
    }
    Ok(snapshot(&app))
}

#[tauri::command]
pub fn desktop_set_shell_installer_auto_download(
    window: WebviewWindow,
    app: AppHandle,
    enabled: bool,
) -> Result<ShellInstallerStatus, String> {
    require_main(&window)?;
    let directory = installers_dir(&app)?;
    pisper_component_updater::shell_installer::write_settings(
        &directory,
        ShellInstallerSettings {
            auto_download: enabled,
        },
    )
    .map_err(|error| format!("{error:#}"))?;
    log_shell_installer(
        &app,
        if enabled {
            "automatic installer download enabled"
        } else {
            "automatic installer download disabled"
        },
    );
    // 刚打开且已知有可下载的新版本：不用等下一轮定时检查。
    let status = snapshot(&app);
    if enabled && status.state == "available" && status.can_download {
        begin(&app, Trigger::Download);
    }
    Ok(snapshot(&app))
}

/// 已验签的安装包路径；打开前再验一次，防止下载后被替换。
async fn verified_path(app: &AppHandle) -> Result<DownloadedInstaller, String> {
    let installer = app
        .state::<ShellInstallerState>()
        .downloaded()
        .ok_or_else(|| "No downloaded installer is available".to_string())?;
    let updater = updater(app)?;
    let current = shell_version(app);
    let version = installer.version.clone();
    blocking(move || updater.verified_installer(&current))
        .await
        .flatten()
        .filter(|verified| verified.version == version)
        .ok_or_else(|| {
            app.state::<ShellInstallerState>().set_downloaded(None);
            set_status(app, |status| {
                status.state = "error".into();
                status.message = "The downloaded installer failed verification".into();
            });
            "The downloaded installer failed verification".to_string()
        })
}

/// “安装”：用系统打开安装包（Windows 运行安装程序，macOS 挂载 dmg，Linux 交给文件关联）。
/// 安装由用户自己完成；Pisper 不退出、不传参数。
#[tauri::command]
pub async fn desktop_open_shell_installer(
    window: WebviewWindow,
    app: AppHandle,
) -> Result<bool, String> {
    require_main(&window)?;
    let installer = verified_path(&app).await?;
    log_shell_installer(&app, &format!("opening {}", installer.file_name));
    app.opener()
        .open_path(installer.path.to_string_lossy().into_owned(), None::<&str>)
        .map(|_| true)
        .map_err(|_| "Failed to open the installer".to_string())
}

#[tauri::command]
pub fn desktop_reveal_shell_installer(
    window: WebviewWindow,
    app: AppHandle,
) -> Result<bool, String> {
    require_main(&window)?;
    let installer = app
        .state::<ShellInstallerState>()
        .downloaded()
        .filter(|installer| installer.path.is_file())
        .ok_or_else(|| "No downloaded installer is available".to_string())?;
    app.opener()
        .reveal_item_in_dir(&installer.path)
        .map(|_| true)
        .map_err(|_| "Failed to show the installer folder".to_string())
}
